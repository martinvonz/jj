/*
** libfault -- small library for crash diagnostics.
**
** Copyright (C) 2014 Austin Seipp.
** Copyright (c) 2010-2014 Phusion (inspired by Phusion Passenger)
** Copyright (c) 2009-2012, Salvatore Sanfilippo (register/stack dumping)
**
** Redistribution and use in source and binary forms, with or without
** modification, are permitted provided that the following conditions are met:
**
**   * Redistributions of source code must retain the above copyright notice,
**     this list of conditions and the following disclaimer.
**   * Redistributions in binary form must reproduce the above copyright
**     notice, this list of conditions and the following disclaimer in the
**     documentation and/or other materials provided with the distribution.
**   * Neither the name of Redis nor the names of its contributors may be used
**     to endorse or promote products derived from this software without
**     specific prior written permission.
**
** THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
** AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
** IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
** ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE
** LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
** CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
** SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
** INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
** CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
** ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
** POSSIBILITY OF SUCH DAMAGE.
**
** [ BSD3 license: http://opensource.org/licenses/bsd-3-clause ]
*/

/*
** Basic theory of operation
** ~~~~~~~~~~~~~~~~~~~~~~~~~
**
** This code has mostly been taken from Phusion Passenger (and massaged into my
** coding style, naming conventions, an API, etc), along with some auxiliary
** code from Redis too, and other extensions.. It has some important principles:
**
**   - All code is async signal-safe.
**
**   - Catches SIGSEGV, SIGABRT, SIGILL, SIGBUS, SIGFPE.
**
**   - Runs the signal handler in a separate, pre-allocated stack using
**     sigaltstack(), just in case the crash occurs because you went over stack
**     boundaries.
**
**   - Reports time and PID of the crashing process.
**
**   - Forks off a child process for gathering most crash report
**     information. This is because we discovered not all operating systems
**     allow signal handlers to do a lot of stuff, even if your code is async
**     signal safe. For example if you try to waitpid() in a SIGSEGV handler on
**     OS X, the kernel just terminates your process.
**
**   - Calls fork() on Linux directly using syscall() because the glibc fork()
**     wrapper tries to grab the ptmalloc2 lock. This will deadlock if it was
**     the memory allocator that crashed.
**
**   - Prints a backtrace upon crash, using backtrace_symbols_fd(). We
**     explicitly do not use backtrace() because the latter may malloc() memory,
**     and that is not async signal safe (it could be memory allocator crashing
**     for all you know!)
**
**   - Pipes the output of backtrace_symbols_fd() to an external script that
**     demangels C++ symbols into sane, readable symbols.
**
**   - Works around OS X-specific signal-threading quirks.
**
**   - Optionally invokes a beep. Useful in developer mode for grabbing the
**     developer's attention.
**
**   - Optionally dumps the entire crash report to a file *in addition* to
**     writing to stderr.
**
**   - Dumps the process memory map.
**
**   - Dumps the registers and stack of the thread that triggered the fault.
**
**   - Full IA32/AMD64/ARM support.
**
**   - Hooks assert() handlers to report assert information in the logs.
**
**   - Gathers program-specific debugging information, e.g. runtime state. You
**     can supply a custom callback to do this.
**
**   - Places a time limit on the crash report gathering code. Because the
**     gathering code may allocate memory or doing other async signal unsafe
**     stuff you never know whether it will crash or deadlock. We give it a few
**     seconds at most to gather information.
**
**   - Allows you to specify a URL/email address for reports to be sent to for
**     users.
**
** See http://www.reddit.com/r/programming/comments/13vmik/redis_crashes_a_small_rant_about_software/c783lzx
** for more information from Hongli Lai.
*/

/*
** TODOS (marked by 'TODO FIXME'):
**
**  - crash-dump support, like in Phusion Passenger.
**
**  - setvbuf nonsense
**
** - reimplement ignore-SIGPIPE behavior on OS X, so as not to trip up
**   `backtrace_symbols_fd()`
**
** Random additions:
**
**  - cat /proc/self/status on Linux perhaps?
*/

#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif

#include <stdio.h>
#include <stdlib.h>
#include <stdbool.h>
#include <stdint.h>
#include <string.h>
#include <errno.h>
#include <assert.h>
#include <fcntl.h>
#include <poll.h>
#include <unistd.h>
#include <pthread.h>
#include <signal.h>
#include <libgen.h>
#include <dirent.h>
#include <limits.h>
#include <sys/types.h>
#include <sys/stat.h>
#include <sys/select.h>
#include <sys/wait.h>
#include <sys/resource.h>
#include <ucontext.h>

#ifdef __linux__
#include <sys/syscall.h>
#include <features.h>
#endif

#if defined(__APPLE__) || defined(__linux__)
#define LIBFAULT_LIBC_HAS_BACKTRACE_FUNC
#include <execinfo.h>
#endif

#include "libfault.h"

/* -------------------------------------------------------------------------- */
/* -- Macros, types, decls -------------------------------------------------- */

#define LIBFAULT_UNUSED __attribute__((unused))

typedef struct libfault_assert_info {
  const char* filename;
  const char* function; /* May be NULL. */
  const char* expression;
  unsigned int line;
} libfault_assert_info;

typedef struct libfault_handler_state {
  pid_t pid;
  int signo;
  siginfo_t* info;
  char msg_prefix[32];
  char msg_buffer[1024];
  ucontext_t* uc;
} libfault_handler_state;

typedef struct libfault_ctx {
  char* sanitizer_cmd;
  bool sanitizer_prog_info;
  char*** orig_argv;
  int orig_argc;
  libfault_custom_diagnostics diagnostics;
  void* diagnostics_data;
  const char* app_name;
  const char* app_version;
  const char* log_name;
  const char* bugreport_url;
} libfault_ctx;

static libfault_ctx libfault_main_ctx;

typedef void (*libfault_callback)(libfault_handler_state* state, void* data);

static const char libfault_ascii_digits[]
    = { '0', '1', '2', '3', '4', '5', '6', '7', '8', '9' };
static const char libfault_ascii_chars[] = "0123456789abcdef";

static libfault_assert_info libfault_last_assert_info;

static libfault_custom_diagnostics libfault_custom_diagnostics_dumper = NULL;
static void* libfault_custom_diagnostics_dumper_data;

static bool libfault_beep_on_abort = false;
static bool libfault_stop_on_abort = false;

/* We preallocate a few pipes during startup which we will close in the crash
** handler. This way we can be sure that when the crash handler calls pipe() it
** won't fail with "Too many files". */
static int emergencyPipe1[2] = { -1, -1 };
static int emergencyPipe2[2] = { -1, -1 };

static volatile unsigned int libfault_abort_handler_called = 0;

static const char* libfault_backtrace_sanitizer_cmd = NULL;
static bool libfault_backtrace_sanitizer_pass_program_info = true;

static const char* libfault_app_name = NULL;
static const char* libfault_app_version = NULL;

static const char* libfault_bugreport_url = NULL;

static const char* libfault_log_base = NULL;

static char** libfault_orig_argv = NULL;

static void libfault_reset_signal_handlers_and_mask();

/* -------------------------------------------------------------------------- */
/* -- Helpful utilities ----------------------------------------------------- */

/**
 * libfault_env_get(str, default):
 *
 * Lorem ipsum...
 *
 * - Returns FOO if BAR.
 * - Returns BAZ under normal circumstances.
 */
static const char*
libfault_env_get(const char* name, const char* def)
{
  const char* val = getenv(name);

  if (val != NULL && *val != '\0') {
    return val;
  }

  return def;
}

static bool
libfault_env_enabled(const char* name, bool def)
{
  const char* val = libfault_env_get(name, NULL);

  if (val != NULL) {
    return strcmp(val, "yes") == 0 || strcmp(val, "YES") == 0
           || strcmp(val, "y") == 0 || strcmp(val, "Y") == 0
           || strcmp(val, "on") == 0 || strcmp(val, "ON") == 0
           || strcmp(val, "true") == 0 || strcmp(val, "TRUE") == 0;
  }

  return def;
}

static size_t
libfault_safe_strlen(const char* str)
{
  size_t sz = 0;
  char* p = (char*)str;

  for (; *p != '\0'; p++) sz++;
  return sz;
}

static void
libfault_write_nowarn(const int fd, const void* buf, size_t n)
{
  ssize_t r = write(fd, buf, n);
  (void)r;
}

static void
libfault_write_err(const void* buf, size_t n)
{
  libfault_write_nowarn(STDERR_FILENO, buf, n);
}

static void
libfault_safe_print(const char* msg)
{
  libfault_write_err(msg, libfault_safe_strlen(msg));
}

static char*
libfault_append_text(char* buf, const char* text)
{
  size_t len = libfault_safe_strlen(text);
  strcpy(buf, text);
  return buf + len;
}

static void
libfault_reverse(char* str, size_t len)
{
  char* p1, *p2;
  if (*str == '\0') {
    return;
  }
  for (p1 = str, p2 = str + len - 1; p2 > p1; ++p1, --p2) {
    *p1 ^= *p2;
    *p2 ^= *p1;
    *p1 ^= *p2;
  }
}

static char*
libfault_append_ull(char* buf, unsigned long long value)
{
  unsigned long long remainder = value;
  unsigned int size = 0;

  do {
    buf[size] = libfault_ascii_digits[remainder % 10];
    remainder = remainder / 10;
    size++;
  } while (remainder != 0);

  libfault_reverse(buf, size);
  return buf + size;
}

static char*
libfault_append_hex_uint(char* buf, unsigned int value)
{
  unsigned int remainder = value;
  unsigned int size = 0;
  unsigned int off = 0;
  unsigned int i = 0;

  do {
    buf[size] = libfault_ascii_chars[remainder % 16];
    remainder = remainder / 16;
    size++;
  } while (remainder != 0);
  off = size;

  for (i = 0; i < (sizeof(unsigned int) * 2) - off; i++) {
    buf[size] = '0';
    size++;
  }

  libfault_reverse(buf, size);
  return buf + size;
}

static char*
libfault_append_hex_ull(char* buf, unsigned long long value)
{
  unsigned long long remainder = value;
  unsigned int size = 0;
  unsigned int off = 0;
  unsigned int i = 0;

  do {
    buf[size] = libfault_ascii_chars[remainder % 16];
    remainder = remainder / 16;
    size++;
  } while (remainder != 0);
  off = size;

  for (i = 0; i < (sizeof(unsigned long long) * 2) - off; i++) {
    buf[size] = '0';
    size++;
  }

  libfault_reverse(buf, size);
  return buf + size;
}

static char*
libfault_append_hex_ul(char* buf, unsigned long value)
{
  unsigned long remainder = value;
  unsigned int size = 0;
  unsigned int off = 0;
  unsigned int i = 0;

  do {
    buf[size] = libfault_ascii_chars[remainder % 16];
    remainder = remainder / 16;
    size++;
  } while (remainder != 0);
  off = size;

  for (i = 0; i < (sizeof(unsigned long) * 2) - off; i++) {
    buf[size] = '0';
    size++;
  }

  libfault_reverse(buf, size);
  return buf + size;
}

static char*
libfault_append_ptr2str(char* buf, void* pointer)
{
  /* NB: Use wierd union construction to avoid compiler warnings. */

  if (sizeof(void*) == sizeof(unsigned int)) {
    union {
      void* pointer;
      unsigned int value;
    } u;
    u.pointer = pointer;
    return libfault_append_hex_uint(libfault_append_text(buf, "0x"), u.value);
  }
  else if (sizeof(void*) == sizeof(unsigned long long)) {
    union {
      void* pointer;
      unsigned long long value;
    } u;
    u.pointer = pointer;
    return libfault_append_hex_ull(libfault_append_text(buf, "0x"), u.value);
  }

  else {
    return libfault_append_text(buf, "(pointer size unsupported)");
  }
}

static char*
libfault_append_signo(char* buf, int signo)
{
  switch (signo) {
  case SIGABRT: buf = libfault_append_text(buf, "SIGABRT"); break;
  case SIGSEGV: buf = libfault_append_text(buf, "SIGSEGV"); break;
  case SIGBUS: buf = libfault_append_text(buf, "SIGBUS"); break;
  case SIGFPE: buf = libfault_append_text(buf, "SIGFPE"); break;
  case SIGILL: buf = libfault_append_text(buf, "SIGILL"); break;
  default: return libfault_append_ull(buf, (unsigned long long)signo);
  }

  buf = libfault_append_text(buf, "(");
  buf = libfault_append_ull(buf, (unsigned long long)signo);
  buf = libfault_append_text(buf, ")");
  return buf;
}

#define LIBFAULT_SI_CODE_HANDLER(name)                                         \
  case name:                                                                   \
    buf = libfault_append_text(buf, #name);                                    \
    break

static char*
libfault_append_sigreason(char* buf, siginfo_t* info)
{
  bool handled = true;

  switch (info->si_code) {
    LIBFAULT_SI_CODE_HANDLER(SI_USER);
    LIBFAULT_SI_CODE_HANDLER(SI_QUEUE);
    LIBFAULT_SI_CODE_HANDLER(SI_TIMER);

/* Possibly non-extant sig codes */
#ifdef SI_KERNEL
    LIBFAULT_SI_CODE_HANDLER(SI_KERNEL);
#endif
#ifdef SI_ASYNCIO
    LIBFAULT_SI_CODE_HANDLER(SI_ASYNCIO);
#endif
#ifdef SI_MESGQ
    LIBFAULT_SI_CODE_HANDLER(SI_MESGQ);
#endif
#ifdef SI_SIGIO
    LIBFAULT_SI_CODE_HANDLER(SI_SIGIO);
#endif
#ifdef SI_TKILL
    LIBFAULT_SI_CODE_HANDLER(SI_TKILL);
#endif

  default:
    switch (info->si_signo) {
    case SIGSEGV:
      switch (info->si_code) {
#ifdef SEGV_MAPERR
        LIBFAULT_SI_CODE_HANDLER(SEGV_MAPERR);
#endif
#ifdef SEGV_ACCERR
        LIBFAULT_SI_CODE_HANDLER(SEGV_ACCERR);
#endif
      default: handled = false; break;
      }
      break;

    case SIGBUS:
      switch (info->si_code) {
#ifdef BUS_ADRALN
        LIBFAULT_SI_CODE_HANDLER(BUS_ADRALN);
#endif
#ifdef BUS_ADRERR
        LIBFAULT_SI_CODE_HANDLER(BUS_ADRERR);
#endif
#ifdef BUS_OBJERR
        LIBFAULT_SI_CODE_HANDLER(BUS_OBJERR);
#endif
      default: handled = false; break;
      }
      break;

    default: handled = false; break;
    }

    if (!handled) {
      buf = libfault_append_text(buf, "#");
      buf = libfault_append_ull(buf, (unsigned long long)info->si_code);
    }
    break;
  }

  if (info->si_code <= 0) {
    buf = libfault_append_text(buf, ", signal sent by PID ");
    buf = libfault_append_ull(buf, (unsigned long long)info->si_pid);
    buf = libfault_append_text(buf, " with UID ");
    buf = libfault_append_ull(buf, (unsigned long long)info->si_uid);
  }

  buf = libfault_append_text(buf, ", si_addr=");
  buf = libfault_append_ptr2str(buf, info->si_addr);

  return buf;
}

#undef LIBFAULT_SI_CODE_HANDLER

/* -------------------------------------------------------------------------- */
/* -- Platform-specific code ------------------------------------------------ */

/* Call fork() on Linux directly using syscall(). glibc's fork() wrapper tries
** to grab the ptmalloc2 lock. This will deadlock if it was the memory allocator
** that crashed. */

static pid_t
libfault_safe_fork()
{
#if defined(__linux__)
#if defined(SYS_fork)
  return (pid_t)syscall(SYS_fork);
#else
  return syscall(SYS_clone, SIGCHLD, 0, 0, 0, 0);
#endif

#elif defined(__APPLE__)
  return __fork();
#else
  return fork();
#endif
}

/* Override assert() to add more features and to fix bugs. We save the
** information of the last assertion failure in a global variable so that we can
** print it to the crash diagnostics report.
**/

#if defined(__GLIBC__)
__attribute__((__noreturn__)) void __assert_fail(__const char* __assertion,
                                                 __const char* __file,
                                                 unsigned int __line,
                                                 __const char* __function)
{
  libfault_last_assert_info.filename = __file;
  libfault_last_assert_info.line = __line;
  libfault_last_assert_info.function = __function;
  libfault_last_assert_info.expression = __assertion;
  fprintf(stderr, "Assertion failed! %s:%u: %s: %s\n", __file, __line,
          __function, __assertion);
  fflush(stderr);
  abort();
}
#endif /* defined(__GLIBC__) */

/**
 * Async-signal safe way to get the current process's hard file descriptor
 * limit.
 */
static int
libfault_get_fd_limit()
{
  long long sysconfResult = sysconf(_SC_OPEN_MAX);
  struct rlimit rl;
  long long rlimitResult;

  if (getrlimit(RLIMIT_NOFILE, &rl) == -1) {
    rlimitResult = 0;
  }
  else {
    rlimitResult = (long long)rl.rlim_max;
  }

  long result;
  /* OS X 10.9 returns LLONG_MAX. It doesn't make sense to use that result
  ** so we limit ourselves to the sysconf result. */
  if (rlimitResult >= INT_MAX || sysconfResult > rlimitResult) {
    result = sysconfResult;
  }
  else {
    result = rlimitResult;
  }

  if (result < 0) {
    /* Unable to query the file descriptor limit. */
    result = 9999;
  }
  else if (result < 2) {
    /* The calls reported broken values. */
    result = 2;
  }
  return result;
}

/**
 * Async-signal safe function to get the highest file descriptor that the
 * process is currently using. See also
 * http://stackoverflow.com/questions/899038/getting-the-highest-allocated-file-descriptor
 */
static int
libfault_get_highest_fd(bool safe)
{
#if defined(F_MAXFD)
  int ret;

  do {
    ret = fcntl(0, F_MAXFD);
  } while (ret == -1 && errno == EINTR);
  if (ret == -1) {
    ret = libfault_get_fd_limit();
  }
  return ret;

#else
  int p[2], ret, flags;
  pid_t pid = -1;
  int result = -1;

  /* Since opendir() may not be async signal safe and thus may lock up
   * or crash, we use it in a child process which we kill if we notice
   * that things are going wrong.
   */

  /* Make a pipe. */
  p[0] = p[1] = -1;
  do {
    ret = pipe(p);
  } while (ret == -1 && errno == EINTR);
  if (ret == -1) {
    goto done;
  }

  /* Make the read side non-blocking. */
  do {
    flags = fcntl(p[0], F_GETFL);
  } while (flags == -1 && errno == EINTR);
  if (flags == -1) {
    goto done;
  }
  do {
    fcntl(p[0], F_SETFL, flags | O_NONBLOCK);
  } while (ret == -1 && errno == EINTR);
  if (ret == -1) {
    goto done;
  }

  if (safe) {
    do {
      pid = libfault_safe_fork();
    } while (pid == -1 && errno == EINTR);
  }
  else {
    do {
      pid = fork();
    } while (pid == -1 && errno == EINTR);
  }

  if (pid == 0) {
    /* Don't close p[0] here or it might affect the result. */
    libfault_reset_signal_handlers_and_mask();

    struct sigaction action;
    action.sa_handler = _exit;
    action.sa_flags = SA_RESTART;
    sigemptyset(&action.sa_mask);
    sigaction(SIGSEGV, &action, NULL);
    sigaction(SIGPIPE, &action, NULL);
    sigaction(SIGBUS, &action, NULL);
    sigaction(SIGILL, &action, NULL);
    sigaction(SIGFPE, &action, NULL);
    sigaction(SIGABRT, &action, NULL);

    DIR* dir = NULL;
#ifdef __APPLE__
    /* /dev/fd can always be trusted on OS X. */
    dir = opendir("/dev/fd");
#else
    /* On FreeBSD and possibly other operating systems, /dev/fd only
     * works if fdescfs is mounted. If it isn't mounted then /dev/fd
     * still exists but always returns [0, 1, 2] and thus can't be
     * trusted. If /dev and /dev/fd are on different filesystems
     * then that probably means fdescfs is mounted.
     */
    struct stat dirbuf1, dirbuf2;
    if (stat("/dev", &dirbuf1) == -1 || stat("/dev/fd", &dirbuf2) == -1) {
      _exit(1);
    }
    if (dirbuf1.st_dev != dirbuf2.st_dev) {
      dir = opendir("/dev/fd");
    }
#endif
    if (dir == NULL) {
      dir = opendir("/proc/self/fd");
      if (dir == NULL) {
        _exit(1);
      }
    }
    struct dirent* ent;
    union {
      int highest;
      char data[sizeof(int)];
    } u;
    u.highest = -1;

    while ((ent = readdir(dir)) != NULL) {
      if (ent->d_name[0] != '.') {
        int number = atoi(ent->d_name);
        if (number > u.highest) {
          u.highest = number;
        }
      }
    }
    if (u.highest != -1) {
      ssize_t ret, written = 0;
      do {
        ret = write(p[1], u.data + written, sizeof(int) - written);
        if (ret == -1) {
          _exit(1);
        }
        written += ret;
      } while (written < (ssize_t)sizeof(int));
    }
    closedir(dir);
    _exit(0);
  }
  else if (pid == -1) {
    goto done;
  }
  else {
    close(p[1]); /* Do not retry on EINTR:
                    http://news.ycombinator.com/item?id=3363819 */
    p[1] = -1;

    union {
      int highest;
      char data[sizeof(int)];
    } u;
    ssize_t ret, bytesRead = 0;
    struct pollfd pfd;
    pfd.fd = p[0];
    pfd.events = POLLIN;

    do {
      do {
        // The child process must finish within 30 ms, otherwise
        // we might as well query sysconf.
        ret = poll(&pfd, 1, 30);
      } while (ret == -1 && errno == EINTR);
      if (ret <= 0) {
        goto done;
      }

      do {
        ret = read(p[0], u.data + bytesRead, sizeof(int) - bytesRead);
      } while (ret == -1 && ret == EINTR);
      if (ret == -1) {
        if (errno != EAGAIN) {
          goto done;
        }
      }
      else if (ret == 0) {
        goto done;
      }
      else {
        bytesRead += ret;
      }
    } while (bytesRead < (ssize_t)sizeof(int));

    result = u.highest;
    goto done;
  }

done:
  /* Do not retry on EINTR: http://news.ycombinator.com/item?id=3363819 */
  if (p[0] != -1) {
    close(p[0]);
  }
  if (p[1] != -1) {
    close(p[1]);
  }
  if (pid != -1) {
    do {
      ret = kill(pid, SIGKILL);
    } while (ret == -1 && errno == EINTR);
    do {
      ret = waitpid(pid, NULL, 0);
    } while (ret == -1 && errno == EINTR);
  }

  if (result == -1) {
    result = libfault_get_fd_limit();
  }
  return result;
#endif
}

static void
libfault_close_all_fds(int last, bool safe)
{
#if defined(F_CLOSEM)
  int ret;
  do {
    ret = fcntl(last + 1, F_CLOSEM);
  } while (ret == -1 && errno == EINTR);
  if (ret != -1) {
    return;
  }

#elif defined(LIBFAULT_HAS_CLOSEFROM)
  closefrom(last + 1);
  return;
#endif

  for (int i = libfault_get_highest_fd(safe); i > last; i--) {
    /* Even though we normally shouldn't retry on EINTR
    ** (http://news.ycombinator.com/item?id=3363819)
    ** it's okay to do that here because because this function
    ** may only be called in a single-threaded environment. */
    int ret;
    do {
      ret = close(i);
    } while (ret == -1 && errno == EINTR);
  }
}

/* -------------------------------------------------------------------------- */
/* -- Process management ---------------------------------------------------- */

static int
libfault_run_subprocess(libfault_handler_state* state,
                        libfault_callback callback, void* userData,
                        int timeLimit)
{
  char* end;
  pid_t child;
  int p[2], e;

  if (pipe(p) == -1) {
    e = errno;
    end = state->msg_buffer;
    end = libfault_append_text(
        end, "Could not create subprocess: pipe() failed with errno=");
    end = libfault_append_ull(end, e);
    end = libfault_append_text(end, "\n");
    libfault_write_err(state->msg_buffer, end - state->msg_buffer);
    return -1;
  }

  child = libfault_safe_fork();
  if (child == 0) {
    close(p[0]);
    callback(state, userData);
    _exit(0);
    return -1;
  }
  else if (child == -1) {
    e = errno;
    close(p[0]);
    close(p[1]);
    end = state->msg_buffer;
    end = libfault_append_text(
        end, "Could not create subprocess: fork() failed with errno=");
    end = libfault_append_ull(end, e);
    end = libfault_append_text(end, "\n");
    libfault_write_err(state->msg_buffer, end - state->msg_buffer);
    return -1;
  }
  else {
    int status;
    close(p[1]);

    /* We give the child process a time limit. If it doesn't succeed in exiting
    ** within the time limit, we assume that it has frozen and we kill it. */
    struct pollfd fd;
    fd.fd = p[0];
    fd.events = POLLIN | POLLHUP | POLLERR;
    if (poll(&fd, 1, timeLimit) <= 0) {
      kill(child, SIGKILL);
      libfault_safe_print(
          "Could not run child process: it did not exit in time\n");
    }
    close(p[0]);
    if (waitpid(child, &status, 0) == child) {
      return status;
    }
    else {
      return -1;
    }
  }
}

/* -------------------------------------------------------------------------- */
/* -- Message dumping code -------------------------------------------------- */

static void
libfault_dump_stack(libfault_handler_state* state, void* data LIBFAULT_UNUSED)
{
  char* messageBuf = state->msg_buffer;
  char* end;
  ucontext_t* uc = state->uc;
  void** stack = NULL;
  int i;

  end = messageBuf;
  end = libfault_append_text(end, "--------------------------------------\n");
  end = libfault_append_text(end, state->msg_prefix);
  end = libfault_append_text(end, " ] Stack dump (16 words)\n");

/* -- Linux ABI ------------------------------------------------------------- */
#if defined(__linux__)
#if defined(__i386__)
  stack = (void**)uc->uc_mcontext.gregs[7];
#elif defined(__X86_64__) || defined(__x86_64__)
  stack = (void**)uc->uc_mcontext.gregs[15];
#elif defined(__arm__)
  stack = (void**)uc->uc_mcontext.arm_sp;
#endif
#endif

  if (stack == NULL) {
    /* No support */
    end = libfault_append_text(
        end, "Stack dumps aren't supported on this platform.\n");
  }
  else {
    for (i = 15; i >= 0; i--) {
      unsigned long addr = (unsigned long)(stack + i);
      unsigned long val = (unsigned long)stack[i];

      end = libfault_append_text(end, "(");
      end = libfault_append_hex_ul(libfault_append_text(end, "0x"),
                                   (unsigned long)addr);
      end = libfault_append_text(end, ") -> (");
      end = libfault_append_hex_ul(libfault_append_text(end, "0x"),
                                   (unsigned long)val);
      end = libfault_append_text(end, ")\n");
    }
  }

  end = libfault_append_text(end, "--------------------------------------\n");

  libfault_write_err(messageBuf, end - messageBuf);

  _exit(1);
}

static void
libfault_dump_registers(libfault_handler_state* state, void* data LIBFAULT_UNUSED)
{
  char* messageBuf = state->msg_buffer;
  char* end;
  ucontext_t* uc = state->uc;

  end = messageBuf;
  end = libfault_append_text(end, "--------------------------------------\n");
  end = libfault_append_text(end, state->msg_prefix);
  end = libfault_append_text(end, " ] Register dump\n");

#define LF_TEXT(x) end = libfault_append_text(end, x)
#define LF_UL(x)                                                               \
  end = libfault_append_hex_ul(libfault_append_text(end, "0x"),                \
                               (unsigned long)x)
#define LF_SPC end = libfault_append_text(end, " ")
#define LF_NL end = libfault_append_text(end, "\n")

/* Linux ABI */
#if defined(__linux__)

/* -------------------------------------------------------------------------- */

#if defined(__i386__)
  LF_TEXT("EAX:");
  LF_UL(uc->uc_mcontext.gregs[11]);
  LF_SPC;
  LF_TEXT("EBX:");
  LF_UL(uc->uc_mcontext.gregs[8]);
  LF_SPC;
  LF_TEXT("ECX:");
  LF_UL(uc->uc_mcontext.gregs[10]);
  LF_SPC;
  LF_TEXT("EDX:");
  LF_UL(uc->uc_mcontext.gregs[9]);
  LF_NL;

  LF_TEXT("EDI:");
  LF_UL(uc->uc_mcontext.gregs[4]);
  LF_SPC;
  LF_TEXT("ESI:");
  LF_UL(uc->uc_mcontext.gregs[5]);
  LF_SPC;
  LF_TEXT("EBP:");
  LF_UL(uc->uc_mcontext.gregs[6]);
  LF_SPC;
  LF_TEXT("ESP:");
  LF_UL(uc->uc_mcontext.gregs[7]);
  LF_NL;

  LF_TEXT("SS :");
  LF_UL(uc->uc_mcontext.gregs[18]);
  LF_SPC;
  LF_TEXT("EFL:");
  LF_UL(uc->uc_mcontext.gregs[17]);
  LF_SPC;
  LF_TEXT("EIP:");
  LF_UL(uc->uc_mcontext.gregs[14]);
  LF_SPC;
  LF_TEXT("CS :");
  LF_UL(uc->uc_mcontext.gregs[15]);
  LF_NL;

  LF_TEXT("DS :");
  LF_UL(uc->uc_mcontext.gregs[3]);
  LF_SPC;
  LF_TEXT("ES :");
  LF_UL(uc->uc_mcontext.gregs[2]);
  LF_SPC;
  LF_TEXT("FS :");
  LF_UL(uc->uc_mcontext.gregs[1]);
  LF_SPC;
  LF_TEXT("GS :");
  LF_UL(uc->uc_mcontext.gregs[0]);
  LF_NL;

/* -------------------------------------------------------------------------- */

#elif defined(__X86_64__) || defined(__x86_64__)
  LF_TEXT("RAX:");
  LF_UL(uc->uc_mcontext.gregs[13]);
  LF_SPC;
  LF_TEXT("RBX:");
  LF_UL(uc->uc_mcontext.gregs[11]);
  LF_NL;

  LF_TEXT("RCX:");
  LF_UL(uc->uc_mcontext.gregs[14]);
  LF_SPC;
  LF_TEXT("RDX:");
  LF_UL(uc->uc_mcontext.gregs[12]);
  LF_NL;

  LF_TEXT("RDI:");
  LF_UL(uc->uc_mcontext.gregs[8]);
  LF_SPC;
  LF_TEXT("RSI:");
  LF_UL(uc->uc_mcontext.gregs[9]);
  LF_NL;

  LF_TEXT("RBP:");
  LF_UL(uc->uc_mcontext.gregs[10]);
  LF_SPC;
  LF_TEXT("RSP:");
  LF_UL(uc->uc_mcontext.gregs[15]);
  LF_NL;

  LF_TEXT("R8 :");
  LF_UL(uc->uc_mcontext.gregs[0]);
  LF_SPC;
  LF_TEXT("R9 :");
  LF_UL(uc->uc_mcontext.gregs[1]);
  LF_NL;

  LF_TEXT("R10:");
  LF_UL(uc->uc_mcontext.gregs[2]);
  LF_SPC;
  LF_TEXT("R11:");
  LF_UL(uc->uc_mcontext.gregs[3]);
  LF_NL;

  LF_TEXT("R12:");
  LF_UL(uc->uc_mcontext.gregs[4]);
  LF_SPC;
  LF_TEXT("R13:");
  LF_UL(uc->uc_mcontext.gregs[5]);
  LF_NL;

  LF_TEXT("R14:");
  LF_UL(uc->uc_mcontext.gregs[6]);
  LF_SPC;
  LF_TEXT("R15:");
  LF_UL(uc->uc_mcontext.gregs[7]);
  LF_NL;

  LF_TEXT("RIP:");
  LF_UL(uc->uc_mcontext.gregs[16]);
  LF_SPC;
  LF_TEXT("EFL:");
  LF_UL(uc->uc_mcontext.gregs[17]);
  LF_NL;
  LF_TEXT("CGF:");
  LF_UL(uc->uc_mcontext.gregs[18]);
  LF_NL; /* CS/GS/FS */

#elif defined(__arm__)
  LF_TEXT("R0:");
  LF_UL(uc->uc_mcontext.arm_r0);
  LF_SPC;
  LF_TEXT("R1:");
  LF_UL(uc->uc_mcontext.arm_r1);
  LF_SPC;
  LF_TEXT("R2:");
  LF_UL(uc->uc_mcontext.arm_r2);
  LF_SPC;
  LF_TEXT(" R3:");
  LF_UL(uc->uc_mcontext.arm_r3);
  LF_NL;

  LF_TEXT("R4:");
  LF_UL(uc->uc_mcontext.arm_r4);
  LF_SPC;
  LF_TEXT("R5:");
  LF_UL(uc->uc_mcontext.arm_r5);
  LF_SPC;
  LF_TEXT("R6:");
  LF_UL(uc->uc_mcontext.arm_r6);
  LF_SPC;
  LF_TEXT(" R7:");
  LF_UL(uc->uc_mcontext.arm_r7);
  LF_NL;

  LF_TEXT("R8:");
  LF_UL(uc->uc_mcontext.arm_r8);
  LF_SPC;
  LF_TEXT("R9:");
  LF_UL(uc->uc_mcontext.arm_r9);
  LF_SPC;
  LF_TEXT("R10:");
  LF_UL(uc->uc_mcontext.arm_r10);
  LF_SPC;
  LF_TEXT("FP:");
  LF_UL(uc->uc_mcontext.arm_fp);
  LF_NL;

  LF_TEXT("IP:");
  LF_UL(uc->uc_mcontext.arm_ip);
  LF_SPC;
  LF_TEXT("SP:");
  LF_UL(uc->uc_mcontext.arm_sp);
  LF_SPC;
  LF_TEXT("LR:");
  LF_UL(uc->uc_mcontext.arm_lr);
  LF_SPC;
  LF_TEXT(" PC:");
  LF_UL(uc->uc_mcontext.arm_pc);
  LF_NL;

  LF_TEXT("CPSR:");
  LF_UL(uc->uc_mcontext.arm_cpsr);
  LF_NL;

#else
  LF_TEXT("Register dumps aren't supported on this Linux architecture.\n");
#endif

/* No support */
#else
  LF_TEXT("Register dumps aren't supported on this platform.\n");
#endif

#undef LF_NL
#undef LF_SPC
#undef LF_UL
#undef LF_TEXT

  libfault_write_err(messageBuf, end - messageBuf);

  _exit(1);
}

static void
libfault_dump_maps(libfault_handler_state* state)
{
  char* messageBuf = state->msg_buffer;
  char* end;
  pid_t pid;
  struct stat buf;
  int status;

  end = messageBuf;
  end = libfault_append_text(end, state->msg_prefix);
  end = libfault_append_text(end, " ] Memory mappings:\n");
  libfault_write_err(messageBuf, end - messageBuf);

  pid = libfault_safe_fork();
  if (pid == 0) {
    libfault_close_all_fds(2, true);

#if defined(__linux__)
    end = messageBuf;
    end = libfault_append_text(end, "/proc/");
    end = libfault_append_ull(end, state->pid);
    end = libfault_append_text(end, "/maps");
    *end = '\0';

    if (stat(messageBuf, &buf) == 0) {
      execlp("cat", "cat", (const char* const)messageBuf, (const char* const)0);
      execlp("/bin/cat", "cat", (const char* const)messageBuf,
             (const char* const)0);
      execlp("/usr/bin/cat", "cat", (const char* const)messageBuf,
             (const char* const)0);
      libfault_safe_print("ERROR: cannot execute 'cat'\n");
    }
    else {
      libfault_safe_print("ERROR: ");
      libfault_safe_print(messageBuf);
      libfault_safe_print(" doesn't exist!\n");
    }
#else
    libfault_safe_print("Memory map dumps aren't supported on this platform\n");
    _exit(0);
#endif

    _exit(1);
  }
  else if (pid == -1) {
    libfault_safe_print("ERROR: Could not fork a process to dump memory map "
                        "information!\n");
  }
  else if (waitpid(pid, &status, 0) != pid || status != 0) {
    libfault_safe_print(
        "ERROR: Could not run 'cat' to dump memory map information!\n");
  }
}

static void
libfault_dump_fds_with_lsof(libfault_handler_state* state, void* data LIBFAULT_UNUSED)
{
  char* end;

  end = state->msg_buffer;
  end = libfault_append_ull(end, state->pid);
  *end = '\0';

  libfault_close_all_fds(2, true);

  execlp("lsof", "lsof", "-p", state->msg_buffer, "-nP", (const char* const)0);

  end = state->msg_buffer;
  end = libfault_append_text(end,
                             "ERROR: cannot execute command 'lsof': errno=");
  end = libfault_append_ull(end, errno);
  end = libfault_append_text(end, "\n");
  libfault_write_err(state->msg_buffer, end - state->msg_buffer);
  _exit(1);
}

static void
libfault_dump_fds_with_ls(libfault_handler_state* state, char* end LIBFAULT_UNUSED)
{
  pid_t pid;
  int status;

  pid = libfault_safe_fork();
  if (pid == 0) {
    libfault_close_all_fds(2, true);

    /* The '-v' is for natual sorting on Linux. On BSD -v means something else
    ** but it's harmless. */
    execlp("ls", "ls", "-lv", state->msg_buffer, (const char* const)0);
    _exit(1);
  }
  else if (pid == -1) {
    libfault_safe_print("ERROR: Could not fork a process to dump file "
                        "descriptor information!\n");
  }
  else if (waitpid(pid, &status, 0) != pid || status != 0) {
    libfault_safe_print(
        "ERROR: Could not run 'ls' to dump file descriptor information!\n");
  }
}

static void
libfault_dump_fds(libfault_handler_state* state)
{
  char* messageBuf = state->msg_buffer;
  char* end;
  struct stat buf;
  int status;

  end = messageBuf;
  end = libfault_append_text(end, state->msg_prefix);
  end = libfault_append_text(end, " ] Open files and file descriptors:\n");
  libfault_write_err(messageBuf, end - messageBuf);

  status
      = libfault_run_subprocess(state, libfault_dump_fds_with_lsof, NULL, 4000);

  if (status != 0) {
    libfault_safe_print(
        "'lsof' not available; falling back to another mechanism for dumping "
        "file descriptors.\n");

    end = messageBuf;
    end = libfault_append_text(end, "/proc/");
    end = libfault_append_ull(end, state->pid);
    end = libfault_append_text(end, "/fd");
    *end = '\0';
    if (stat(messageBuf, &buf) == 0) {
      libfault_dump_fds_with_ls(state, end + 1);
    }
    else {
      end = messageBuf;
      end = libfault_append_text(end, "/dev/fd");
      *end = '\0';
      if (stat(messageBuf, &buf) == 0) {
        libfault_dump_fds_with_ls(state, end + 1);
      }
      else {
        end = messageBuf;
        end = libfault_append_text(end, "ERROR: No other file descriptor "
                                        "dumping mechanism on current platform "
                                        "detected.\n");
        libfault_write_err(messageBuf, end - messageBuf);
      }
    }
  }
}

#ifdef LIBFAULT_LIBC_HAS_BACKTRACE_FUNC
/**
 * Prints a backtrace upon crash, using backtrace_symbols_fd(). We explicitly do
 * not use backtrace_symbols() because the latter may malloc() memory, and that
 * is not async signal safe (it could be memory allocator crashing for all you
 * know!)
 */
static void
libfault_dump_backtrace(libfault_handler_state* state, void* data LIBFAULT_UNUSED)
{
  void* backtraceStore[512];
  int frames
      = backtrace(backtraceStore, sizeof(backtraceStore) / sizeof(void*));
  char* end = state->msg_buffer;
  end = libfault_append_text(end, "[ pid=");
  end = libfault_append_ull(end, (unsigned long long)state->pid);
  end = libfault_append_text(end, " ] Backtrace with ");
  end = libfault_append_ull(end, (unsigned long long)frames);
  end = libfault_append_text(end, " frames:\n");
  libfault_write_err(state->msg_buffer, end - state->msg_buffer);

  if (libfault_backtrace_sanitizer_cmd != NULL) {
    int p[2];
    if (pipe(p) == -1) {
      int e = errno;
      end = state->msg_buffer;
      end = libfault_append_text(end,
                                 "Could not dump diagnostics through backtrace "
                                 "sanitizer: pipe() failed with errno=");
      end = libfault_append_ull(end, e);
      end = libfault_append_text(end, "\n");
      end = libfault_append_text(
          end, "Falling back to writing to stderr directly...\n");
      libfault_write_err(state->msg_buffer, end - state->msg_buffer);
      backtrace_symbols_fd(backtraceStore, frames, STDERR_FILENO);
      return;
    }

    pid_t pid = libfault_safe_fork();
    if (pid == 0) {
      const char* pidStr = end = state->msg_buffer;
      end = libfault_append_ull(end, (unsigned long long)state->pid);
      *end = '\0';
      end++;

      close(p[1]);
      dup2(p[0], STDIN_FILENO);
      libfault_close_all_fds(2, true);

      char* command = end;
      end = libfault_append_text(end, "exec ");
      end = libfault_append_text(end, libfault_backtrace_sanitizer_cmd);
      if (libfault_backtrace_sanitizer_pass_program_info) {
        end = libfault_append_text(end, " \"");
        end = libfault_append_text(end, libfault_orig_argv[0]);
        end = libfault_append_text(end, "\" ");
        end = libfault_append_text(end, pidStr);
      }
      *end = '\0';
      end++;
      execlp("/bin/sh", "/bin/sh", "-c", command, (const char* const)0);

      end = state->msg_buffer;
      end = libfault_append_text(end, "ERROR: cannot execute '");
      end = libfault_append_text(end, libfault_backtrace_sanitizer_cmd);
      end = libfault_append_text(
          end, "' for sanitizing the backtrace, trying 'cat'...\n");
      libfault_write_err(state->msg_buffer, end - state->msg_buffer);
      execlp("cat", "cat", (const char* const)0);
      execlp("/bin/cat", "cat", (const char* const)0);
      execlp("/usr/bin/cat", "cat", (const char* const)0);
      libfault_safe_print("ERROR: cannot execute 'cat'\n");
      _exit(1);
    }
    else if (pid == -1) {
      close(p[0]);
      close(p[1]);
      int e = errno;
      end = state->msg_buffer;
      end = libfault_append_text(end,
                                 "Could not dump diagnostics through backtrace "
                                 "sanitizer: fork() failed with errno=");
      end = libfault_append_ull(end, e);
      end = libfault_append_text(end, "\n");
      end = libfault_append_text(
          end, "Falling back to writing to stderr directly...\n");
      libfault_write_err(state->msg_buffer, end - state->msg_buffer);
      backtrace_symbols_fd(backtraceStore, frames, STDERR_FILENO);
    }
    else {
      int status = -1;

      close(p[0]);
      backtrace_symbols_fd(backtraceStore, frames, p[1]);
      close(p[1]);
      if (waitpid(pid, &status, 0) == -1 || status != 0) {
        end = state->msg_buffer;
        end = libfault_append_text(end, "ERROR: cannot execute '");
        end = libfault_append_text(end, libfault_backtrace_sanitizer_cmd);
        end = libfault_append_text(
            end,
            "' for sanitizing the backtrace, writing to stderr directly...\n");
        libfault_write_err(state->msg_buffer, end - state->msg_buffer);
        backtrace_symbols_fd(backtraceStore, frames, STDERR_FILENO);
      }
    }
  }
  else {
    backtrace_symbols_fd(backtraceStore, frames, STDERR_FILENO);
  }
}
#endif /* LIBFAULT_LIBC_HAS_BACKTRACE_FUNC */

static void
libfault_install_custom_diagnostics(libfault_custom_diagnostics func,
                                    void* data)
{
  libfault_custom_diagnostics_dumper = func;
  libfault_custom_diagnostics_dumper_data = data;
}

static void
libfault_run_custom_diagnostics(libfault_handler_state* state LIBFAULT_UNUSED,
                                void* data LIBFAULT_UNUSED)
{
  libfault_custom_diagnostics_dumper(libfault_custom_diagnostics_dumper_data);
}

static void
libfault_dump_diagnostics(libfault_handler_state* state)
{
  char* messageBuf = state->msg_buffer;
  char* end;
  pid_t pid;
  int status;

  end = messageBuf;
  end = libfault_append_text(end, "--------------------------------------\n");
  libfault_write_err(messageBuf, end - messageBuf);

  /* Dump human-readable time string and string. */
  pid = libfault_safe_fork();
  if (pid == 0) {
    libfault_close_all_fds(2, true);
    execlp("date", "date", (const char* const)0);
    _exit(1);
  }
  else if (pid == -1) {
    libfault_safe_print("ERROR: Could not fork a process to dump the time!\n");
  }
  else if (waitpid(pid, &status, 0) != pid || status != 0) {
    libfault_safe_print("ERROR: Could not run 'date'!\n");
  }

  /* Dump system uname. */
  pid = libfault_safe_fork();
  if (pid == 0) {
    libfault_close_all_fds(2, true);
    execlp("uname", "uname", "-mprsv", (const char* const)0);
    _exit(1);
  }
  else if (pid == -1) {
    libfault_safe_print("ERROR: Could not fork a process to dump the uname!\n");
  }
  else if (waitpid(pid, &status, 0) != pid || status != 0) {
    libfault_safe_print("ERROR: Could not run 'uname -mprsv'!\n");
  }

  /* Dump ulimit. */
  pid = libfault_safe_fork();
  if (pid == 0) {
    libfault_close_all_fds(2, true);
    execlp("ulimit", "ulimit", "-a", (const char* const)0);
    /* On Linux 'ulimit' is a shell builtin, not a command. */
    execlp("/bin/sh", "/bin/sh", "-c", "ulimit -a", (const char* const)0);
    _exit(1);
  }
  else if (pid == -1) {
    libfault_safe_print(
        "ERROR: Could not fork a process to dump the ulimit!\n");
  }
  else if (waitpid(pid, &status, 0) != pid || status != 0) {
    libfault_safe_print("ERROR: Could not run 'ulimit -a'!\n");
  }

  end = messageBuf;

  if (libfault_last_assert_info.filename != NULL) {
    end = messageBuf;
    end = libfault_append_text(end, "--------------------------------------\n");
    end = libfault_append_text(end, state->msg_prefix);
    end = libfault_append_text(end, " ] Last assertion failure: (");
    end = libfault_append_text(end, libfault_last_assert_info.expression);
    end = libfault_append_text(end, "), ");
    if (libfault_last_assert_info.function != NULL) {
      end = libfault_append_text(end, "function ");
      end = libfault_append_text(end, libfault_last_assert_info.function);
      end = libfault_append_text(end, ", ");
    }
    end = libfault_append_text(end, "file ");
    end = libfault_append_text(end, libfault_last_assert_info.filename);
    end = libfault_append_text(end, ", line ");
    end = libfault_append_ull(end, libfault_last_assert_info.line);
    end = libfault_append_text(end, ".\n");
    libfault_write_err(messageBuf, end - messageBuf);
  }

  libfault_run_subprocess(state, libfault_dump_registers, NULL, 2000);
  libfault_run_subprocess(state, libfault_dump_stack, NULL, 2000);

  /* It is important that writing the message and the backtrace are two seperate
  ** operations because it's not entirely clear whether the latter is async
  ** signal safe and thus can crash. */
  end = messageBuf;
  end = libfault_append_text(end, state->msg_prefix);
#ifdef LIBFAULT_LIBC_HAS_BACKTRACE_FUNC
  end = libfault_append_text(end, " ] libc backtrace available!\n");
#else
  end = libfault_append_text(end, " ] libc backtrace not available.\n");
#endif
  libfault_write_err(messageBuf, end - messageBuf);

#ifdef LIBFAULT_LIBC_HAS_BACKTRACE_FUNC
  libfault_run_subprocess(state, libfault_dump_backtrace, NULL, 4000);
#endif

  libfault_safe_print("--------------------------------------\n");

  if (libfault_custom_diagnostics_dumper != NULL) {
    end = messageBuf;
    end = libfault_append_text(end, state->msg_prefix);
    end = libfault_append_text(
        end, " ] Dumping additional diagnostical information...\n");
    libfault_write_err(messageBuf, end - messageBuf);
    libfault_safe_print("--------------------------------------\n");
    libfault_run_subprocess(state, libfault_run_custom_diagnostics, NULL, 2000);
    libfault_safe_print("--------------------------------------\n");
  }

  libfault_dump_maps(state);
  libfault_safe_print("--------------------------------------\n");

  libfault_dump_fds(state);
  libfault_safe_print("--------------------------------------\n");

  /* TODO FIXME
    if (shouldDumpWithCrashWatch) {
      end = messageBuf;
      end = libfault_append_text(end, state->msg_prefix);
  #ifdef LIBFAULT_LIBC_HAS_BACKTRACE_FUNC
      end = libfault_append_text(
          end, " ] Dumping a more detailed backtrace with crash-watch...\n");
  #else
      end = libfault_append_text(end, " ] Dumping a backtrace with
  crash-watch...\n");
  #endif
      libfault_write_err(messageBuf, end - messageBuf);
      dumpWithCrashWatch(state);
    }
  */

  libfault_write_err("\n", 1);
}

/* -------------------------------------------------------------------------- */
/* -- Crash log handling ---------------------------------------------------- */

static bool
libfault_create_crashlog_file(char* filename, time_t t)
{
  char* end = filename;
  end = libfault_append_text(end, libfault_log_base);
  end = libfault_append_ull(end, (unsigned long long)t);
  *end = '\0';

  int fd = open(filename, O_WRONLY | O_CREAT | O_TRUNC, 0600);
  if (fd == -1) {
    *filename = '\0';
    return false;
  }
  else {
    close(fd);
    return true;
  }
}

static void
libfault_fork_and_redir_to_tee(char* filename)
{
  pid_t pid;
  int p[2];

  if (pipe(p) == -1) {
    /* Signal error condition. */
    *filename = '\0';
    return;
  }

  pid = libfault_safe_fork();
  if (pid == 0) {
    close(p[1]);
    dup2(p[0], STDIN_FILENO);
    execlp("tee", "tee", filename, (const char* const)0);
    execlp("/usr/bin/tee", "tee", filename, (const char* const)0);
    execlp("cat", "cat", (const char* const)0);
    execlp("/bin/cat", "cat", (const char* const)0);
    execlp("/usr/bin/cat", "cat", (const char* const)0);
    libfault_safe_print(
        "ERROR: cannot execute 'tee' or 'cat'; crash log will be lost!\n");
    _exit(1);
  }
  else if (pid == -1) {
    libfault_safe_print("ERROR: cannot fork a process for executing 'tee'\n");
    *filename = '\0';
  }
  else {
    close(p[0]);
    dup2(p[1], STDOUT_FILENO);
    dup2(p[1], STDERR_FILENO);
  }
}

/* -------------------------------------------------------------------------- */
/* -- Signal handling code -------------------------------------------------- */

static void
libfault_reset_signal_handlers_and_mask()
{
  struct sigaction action;
  action.sa_handler = SIG_DFL;
  action.sa_flags = SA_RESTART;
  sigemptyset(&action.sa_mask);
  sigaction(SIGHUP, &action, NULL);
  sigaction(SIGINT, &action, NULL);
  sigaction(SIGQUIT, &action, NULL);
  sigaction(SIGILL, &action, NULL);
  sigaction(SIGTRAP, &action, NULL);
  sigaction(SIGABRT, &action, NULL);
#ifdef SIGEMT
  sigaction(SIGEMT, &action, NULL);
#endif
  sigaction(SIGFPE, &action, NULL);
  sigaction(SIGBUS, &action, NULL);
  sigaction(SIGSEGV, &action, NULL);
  sigaction(SIGSYS, &action, NULL);
  sigaction(SIGPIPE, &action, NULL);
  sigaction(SIGALRM, &action, NULL);
  sigaction(SIGTERM, &action, NULL);
  sigaction(SIGURG, &action, NULL);
  sigaction(SIGSTOP, &action, NULL);
  sigaction(SIGTSTP, &action, NULL);
  sigaction(SIGCONT, &action, NULL);
  sigaction(SIGCHLD, &action, NULL);
#ifdef SIGINFO
  sigaction(SIGINFO, &action, NULL);
#endif
  sigaction(SIGUSR1, &action, NULL);
  sigaction(SIGUSR2, &action, NULL);

  /* We reset the signal mask after resetting the signal handlers, because prior
  ** to calling resetSignalHandlersAndMask(), the process might be blocked on
  ** some signals. We want those signals to be processed after installing the
  ** new signal handlers so that bugs like
  ** https://github.com/phusion/passenger/pull/97 can be prevented. */

  sigset_t signal_set;
  int ret;

  sigemptyset(&signal_set);
  do {
    ret = sigprocmask(SIG_SETMASK, &signal_set, NULL);
  } while (ret == -1 && errno == EINTR);
}

static void
libfault_abort_handler(int signo, siginfo_t* info, void* ucontext)
{
  libfault_handler_state state;
  state.pid = getpid();
  state.signo = signo;
  state.info = info;
  state.uc = (ucontext_t*)ucontext;
  pid_t child;
  time_t t = time(NULL);
  char crashLogFile[256];

  libfault_abort_handler_called++;
  if (libfault_abort_handler_called > 1) {
    /* The abort handler itself crashed! */
    char* end = state.msg_buffer;
    end = libfault_append_text(end, "[ origpid=");
    end = libfault_append_ull(end, (unsigned long long)state.pid);
    end = libfault_append_text(end, ", pid=");
    end = libfault_append_ull(end, (unsigned long long)getpid());
    end = libfault_append_text(end, ", timestamp=");
    end = libfault_append_ull(end, (unsigned long long)t);
    if (libfault_abort_handler_called == 2) {
      /* This is the first time it crashed. */
      end = libfault_append_text(end, " ] Abort handler crashed! signo=");
      end = libfault_append_signo(end, state.signo);
      end = libfault_append_text(end, ", reason=");
      end = libfault_append_sigreason(end, state.info);
      end = libfault_append_text(end, "\n");
      libfault_write_err(state.msg_buffer, end - state.msg_buffer);
      /* Run default signal handler. */
      raise(signo);
    }
    else {
      /* This is the second time it crashed, meaning it failed to invoke the
      ** default signal handler to abort the process! */
      end = libfault_append_text(
          end,
          " ] Abort handler crashed again! Force exiting this time. signo=");
      end = libfault_append_signo(end, state.signo);
      end = libfault_append_text(end, ", reason=");
      end = libfault_append_sigreason(end, state.info);
      end = libfault_append_text(end, "\n");
      libfault_write_err(state.msg_buffer, end - state.msg_buffer);
      _exit(1);
    }
    return;
  }

  if (emergencyPipe1[0] != -1) {
    close(emergencyPipe1[0]);
  }
  if (emergencyPipe1[1] != -1) {
    close(emergencyPipe1[1]);
  }
  if (emergencyPipe2[0] != -1) {
    close(emergencyPipe2[0]);
  }
  if (emergencyPipe2[1] != -1) {
    close(emergencyPipe2[1]);
  }
  emergencyPipe1[0] = emergencyPipe1[1] = -1;
  emergencyPipe2[0] = emergencyPipe2[1] = -1;

  /* We want to dump the entire crash log to both stderr and a log file.
   * We use 'tee' for this.
   */
  if (libfault_create_crashlog_file(crashLogFile, t)) {
    libfault_fork_and_redir_to_tee(crashLogFile);
  }

  char* end = state.msg_prefix;
  end = libfault_append_text(end, "[ pid=");
  end = libfault_append_ull(end, (unsigned long long)state.pid);
  *end = '\0';

  end = state.msg_buffer;
  end = libfault_append_text(end, state.msg_prefix);
  end = libfault_append_text(end, ", timestamp=");
  end = libfault_append_ull(end, (unsigned long long)t);
  end = libfault_append_text(end, " ] Process aborted! signo=");
  end = libfault_append_signo(end, state.signo);
  end = libfault_append_text(end, ", reason=");
  end = libfault_append_sigreason(end, state.info);
  /* end = libfault_append_text(end, ", randomSeed="); */
  /* end = libfault_append_ull(end, (unsigned long long)randomSeed); */
  end = libfault_append_text(end, "\n");
  libfault_write_err(state.msg_buffer, end - state.msg_buffer);

  end = state.msg_buffer;

  if (libfault_app_name != NULL) {
    end = libfault_append_text(end, state.msg_prefix);
    end = libfault_append_text(end, " ] Application: ");
    end = libfault_append_text(end, libfault_app_name);
    if (libfault_app_version != NULL) {
      end = libfault_append_text(end, "; version: ");
      end = libfault_append_text(end, libfault_app_version);
      end = libfault_append_text(end, "\n");
    }
  }

  if (libfault_bugreport_url != NULL) {
    end = libfault_append_text(end, state.msg_prefix);
    end = libfault_append_text(
        end, " ] Oops! You've hit a nasty bug in this application.\n");
    end = libfault_append_text(end, state.msg_prefix);
    end = libfault_append_text(end,
                               " ] Please copy this message and send it to\n");
    end = libfault_append_text(end, state.msg_prefix);
    end = libfault_append_text(end, " ]    ");
    end = libfault_append_text(end, libfault_bugreport_url);
    end = libfault_append_text(end, "\n");
  }

  if (*crashLogFile != '\0') {
    end = libfault_append_text(end, state.msg_prefix);
    end = libfault_append_text(end, " ] Crash log dumped to ");
    end = libfault_append_text(end, crashLogFile);
    end = libfault_append_text(end, "\n");
  }
  else {
    end = libfault_append_text(end, state.msg_prefix);
    end = libfault_append_text(
        end,
        " ] Could not create crash log file, so dumping to stderr only.\n");
  }
  libfault_write_err(state.msg_buffer, end - state.msg_buffer);

  if (libfault_beep_on_abort) {
    end = state.msg_buffer;
    end = libfault_append_text(end, state.msg_prefix);
    end = libfault_append_text(
        end, " ] LIBFAULT_BEEP_ON_ABORT on, executing beep...\n");
    libfault_write_err(state.msg_buffer, end - state.msg_buffer);

    child = libfault_safe_fork();
    if (child == 0) {
      libfault_close_all_fds(2, true);

#ifdef __APPLE__
      execlp("osascript", "osascript", "-e", "beep 2", (const char* const)0);
      libfault_safe_print("Cannot execute 'osascript' command\n");
#else
      execlp("beep", "beep", (const char* const)0);
      libfault_safe_print("Cannot execute 'beep' command\n");
#endif
      _exit(1);
    }
    else if (child == -1) {
      int e = errno;
      end = state.msg_buffer;
      end = libfault_append_text(end, state.msg_prefix);
      end = libfault_append_text(end,
                                 " ] Could fork a child process for invoking a "
                                 "beep: fork() failed with errno=");
      end = libfault_append_ull(end, e);
      end = libfault_append_text(end, "\n");
      libfault_write_err(state.msg_buffer, end - state.msg_buffer);
    }
  }

  if (libfault_stop_on_abort) {
    end = state.msg_buffer;
    end = libfault_append_text(end, state.msg_prefix);
    end = libfault_append_text(
        end, " ] LIBFAULT_STOP_ON_ABORT on, so process stopped. "
             "Send SIGCONT when you want to continue.\n");
    libfault_write_err(state.msg_buffer, end - state.msg_buffer);
    raise(SIGSTOP);
  }

  /* It isn't safe to call any waiting functions in this signal handler, not
  ** even read() and waitpid() even though they're async signal safe.  So we
  ** fork a child process and let it dump as much diagnostics as possible
  ** instead of doing it in this process. */
  child = libfault_safe_fork();
  if (child == 0) {
    /* Sleep for a short while to allow the parent process to raise SIGSTOP.
    ** usleep() and nanosleep() aren't async signal safe so we use select()
    ** instead. */
    struct timeval tv;
    tv.tv_sec = 0;
    tv.tv_usec = 100000;
    select(0, NULL, NULL, NULL, &tv);

    libfault_reset_signal_handlers_and_mask();

    child = libfault_safe_fork();
    if (child == 0) {
      /* OS X: for some reason the SIGPIPE handler may be reset to default after
      ** forking. Later in this program we're going to pipe
      ** backtrace_symbols_fd() into the backtrace sanitizer, which may fail,
      ** and we don't want the diagnostics process to crash with SIGPIPE as a
      ** result, so we ignore SIGPIPE again. */
      // TODO FIXME
      // ignoreSigpipe();
      libfault_dump_diagnostics(&state);
      /* The child process may or may or may not resume the original process.
      ** We do it ourselves just to be sure. */
      kill(state.pid, SIGCONT);
      _exit(0);
    }
    else if (child == -1) {
      int e = errno;
      end = state.msg_buffer;
      end = libfault_append_text(end, state.msg_prefix);
      end = libfault_append_text(end,
                                 "] Could fork a child process for dumping "
                                 "diagnostics: fork() failed with errno=");
      end = libfault_append_ull(end, e);
      end = libfault_append_text(end, "\n");
      libfault_write_err(state.msg_buffer, end - state.msg_buffer);
      _exit(1);
    }
    else {
      /* Exit immediately so that child process is adopted by init process. */
      _exit(0);
    }
  }
  else if (child == -1) {
    int e = errno;
    end = state.msg_buffer;
    end = libfault_append_text(end, state.msg_prefix);
    end = libfault_append_text(end, " ] Could fork a child process for dumping "
                                    "diagnostics: fork() failed with errno=");
    end = libfault_append_ull(end, e);
    end = libfault_append_text(end, "\n");
    libfault_write_err(state.msg_buffer, end - state.msg_buffer);
  }
  else {
    raise(SIGSTOP);
    /* Will continue after the child process has done its job. */
  }

  /* Run default signal handler. */
  raise(signo);
}

static void
libfault_install_abort()
{
  size_t altStkSize = MINSIGSTKSZ + 128 * 1024;
  char* altStk = malloc(altStkSize);

  /* Install new stack for signals */
  stack_t stk;
  stk.ss_sp = altStk;
  stk.ss_size = altStkSize;
  stk.ss_flags = 0;

  if (sigaltstack(&stk, NULL) != 0) {
    int e = errno;
    fprintf(stderr, "Cannot install an alternative stack for use in signal "
                    "handlers: %s (%d)\n",
            strerror(e), e);
    fflush(stderr);
    abort();
  }

  /* Install handler (using previously created stack) */
  struct sigaction act;
  act.sa_sigaction = libfault_abort_handler;
  act.sa_flags = SA_RESETHAND | SA_SIGINFO | SA_ONSTACK;
  sigemptyset(&act.sa_mask);
  sigaction(SIGABRT, &act, NULL);
  sigaction(SIGSEGV, &act, NULL);
  sigaction(SIGBUS, &act, NULL);
  sigaction(SIGFPE, &act, NULL);
  sigaction(SIGILL, &act, NULL);

  return;
}

/* -------------------------------------------------------------------------- */
/* -- Primary user-facing API ----------------------------------------------- */

/**
 * libfault_init():
 *
 * Initializes the libfault library. Must be called before any other
 * functions.
 *
 * - Returns nothing.
 */
void
libfault_init(void)
{
  libfault_main_ctx.sanitizer_cmd = NULL;
  libfault_main_ctx.sanitizer_prog_info = false;
  libfault_main_ctx.orig_argv = NULL;
  libfault_main_ctx.orig_argc = 0;
  libfault_main_ctx.diagnostics = NULL;
  libfault_main_ctx.diagnostics_data = NULL;
  libfault_main_ctx.app_name = NULL;
  libfault_main_ctx.app_version = NULL;
  libfault_main_ctx.log_name = NULL;
  libfault_main_ctx.bugreport_url = NULL;
}

/**
 * libfault_set_app_name(name):
 *
 * Sets the application name to ${name}, which is put into the log
 * files.
 *
 * This can only be called after ${libfault_init}.
 *
 * - Returns nothing.
 */
void
libfault_set_app_name(const char* name)
{
  libfault_main_ctx.app_name = name;
}

/**
 * libfault_set_app_version(version):
 *
 * Sets the application version to ${version}, which is put into the
 * log files.
 *
 * This can only be called after ${libfault_init}.
 *
 * - Returns nothing.
 */
void
libfault_set_app_version(const char* version)
{
  libfault_main_ctx.app_version = version;
}

/**
 * libfault_set_log_name(path):
 *
 * Sets the base filename of crash logs to ${path}. Log paths are
 * created by appending a timestamp to the base filename.
 *
 * By default, libfault puts crash logs under /tmp if possible.
 *
 * This can only be called after ${libfault_init}.
 *
 * - Returns nothing.
 */
void
libfault_set_log_name(const char* path)
{
  libfault_main_ctx.log_name = path;
}

/**
 * libfault_set_bugreport_url(url):
 *
 * Sets the bugreporting URL to ${url}. When a crash occurs, libfault
 * will output this URL into the crash log, so your users can report the bug
 * and information somewhere.
 *
 * This can only be called after ${libfault_init}.
 *
 * - Returns nothing.
 */
void
libfault_set_bugreport_url(const char* url)
{
  libfault_main_ctx.bugreport_url = url;
}

/**
 * libfault_set_custom_diagnostics(callback):
 *
 * Set a custom diagnostics callback. When a crash occurs, libfault
 * will fork the process safely and call the function specified by
 * ${callback}, so it may output custom information.
 *
 * This can only be called after ${libfault_init}.
 *
 * - Returns nothing.
 */
void
libfault_set_custom_diagnostics(libfault_custom_diagnostics callback)
{
  libfault_main_ctx.diagnostics = callback;
}

/**
 * libfault_set_custom_diagnostics_data(data):
 *
 * Set a custom piece of crash diagnostics data; when a crash occurs
 * and a custom diagnostics handler has been set with
 * ${libfault_set_custom_diagnostics}, the specified callback will be
 * given ${data} as an argument.
 *
 * This can only be called after ${libfault_init}.
 *
 * - Returns nothing.
 */
void
libfault_set_custom_diagnostics_data(void* data)
{
  libfault_main_ctx.diagnostics_data = data;
}

/**
 * libfault_install_handlers():
 *
 * Install default libfault signal handlers for crash reporting.
 *
 * This can only be called after ${libfault_init}.
 *
 * - Returns nothing.
 */
void
libfault_install_handlers(void)
{
  if (libfault_env_enabled("LIBFAULT_ABORT_HANDLER", true)) {
    libfault_beep_on_abort
        = libfault_env_enabled("LIBFAULT_BEEP_ON_ABORT", false);
    libfault_stop_on_abort
        = libfault_env_enabled("LIBFAULT_STOP_ON_ABORT", false);
    libfault_install_custom_diagnostics(libfault_main_ctx.diagnostics,
                                        libfault_main_ctx.diagnostics_data);
    libfault_install_abort();
  }

  if (libfault_main_ctx.sanitizer_cmd == NULL) {
    libfault_backtrace_sanitizer_cmd = "c++filt -n";
    libfault_backtrace_sanitizer_pass_program_info = false;
  }

  if (libfault_main_ctx.app_name != NULL)
    libfault_app_name = libfault_main_ctx.app_name;
  if (libfault_main_ctx.app_version != NULL)
    libfault_app_version = libfault_main_ctx.app_version;
  if (libfault_main_ctx.bugreport_url != NULL)
    libfault_bugreport_url = libfault_main_ctx.bugreport_url;

  if (libfault_main_ctx.log_name != NULL) {
    libfault_log_base = libfault_main_ctx.log_name;
  }
  else {
    libfault_log_base = "/tmp/exe-crash.libfault.";
  }

  /* TODO FIXME
  setvbuf(stdout, NULL, _IONBF, 0);
  setvbuf(stderr, NULL, _IONBF, 0);
  */

  libfault_orig_argv
      = (char**)malloc(libfault_main_ctx.orig_argc * sizeof(char*));
  for (int i = 0; i < libfault_main_ctx.orig_argc; i++) {
    libfault_orig_argv[i] = strdup((*libfault_main_ctx.orig_argv)[i]);
  }
  libfault_main_ctx.orig_argv = &libfault_orig_argv;
}

/* -------------------------------------------------------------------------- */
/* -- Shared library entry-point -------------------------------------------- */

#if defined(LIBFAULT_PRELOAD_SHARED_LIBRARY)

__attribute__((constructor)) static void libfault_init_shlib()
{
  libfault_init();
  libfault_install_handlers();
}

#endif /* !defined(LIBFAULT_PRELOAD_SHARED_LIBRARY) */

// Local Variables:
// fill-column: 80
// indent-tabs-mode: nil
// c-basic-offset: 2
// buffer-file-coding-system: utf-8-unix
// c-file-style: "BSD"
// End:
