use std::io;
use std::sync::{Mutex, Once};

use slab::Slab;
use tracing::instrument;

/// Contains the callbacks passed to currently-live [`CleanupGuard`]s
static LIVE_GUARDS: Mutex<GuardTable> = Mutex::new(Slab::new());

type GuardTable = Slab<Box<dyn FnOnce() + Send>>;

/// Prepare to run [`CleanupGuard`]s on `SIGINT`/`SIGTERM`
pub fn init() {
    // Safety: `` ensures at most one call
    static CALLED: Once = Once::new();
    CALLED.call_once(|| {
        if let Err(ref e) = unsafe { platform::init() } {
            eprintln!("couldn't register signal handler: {e}");
        }
    });
}

/// A drop guard that also runs on `SIGINT`/`SIGTERM`
pub struct CleanupGuard {
    slot: usize,
}

impl CleanupGuard {
    /// Invoke `f` when dropped or killed by `SIGINT`/`SIGTERM`
    pub fn new<F: FnOnce() + Send + 'static>(f: F) -> Self {
        let guards = &mut *LIVE_GUARDS.lock().unwrap();
        Self {
            slot: guards.insert(Box::new(f)),
        }
    }
}

impl Drop for CleanupGuard {
    #[instrument(skip_all)]
    fn drop(&mut self) {
        let guards = &mut *LIVE_GUARDS.lock().unwrap();
        let f = guards.remove(self.slot);
        f();
    }
}

#[cfg(unix)]
mod platform {
    use std::os::unix::io::{IntoRawFd as _, RawFd};
    use std::os::unix::net::UnixDatagram;
    use std::panic::AssertUnwindSafe;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    use libc::{c_int, SIGINT, SIGTERM};

    use super::*;

    /// Safety: Must be called at most once
    pub unsafe fn init() -> io::Result<()> {
        let (send, recv) = UnixDatagram::pair()?;

        // Spawn a background thread that waits for the signal handler to write a signal
        // into it
        thread::spawn(move || {
            let mut buf = [0];
            let signal = match recv.recv(&mut buf) {
                Ok(1) => c_int::from(buf[0]),
                _ => unreachable!(),
            };
            // We must hold the lock for the remainder of the process's lifetime to avoid a
            // race where a guard is created between `on_signal` and `raise`.
            let guards = &mut *LIVE_GUARDS.lock().unwrap();
            if let Err(e) = std::panic::catch_unwind(AssertUnwindSafe(|| on_signal(guards))) {
                match e.downcast::<String>() {
                    Ok(s) => eprintln!("signal handler panicked: {s}"),
                    Err(_) => eprintln!("signal handler panicked"),
                }
            }
            libc::signal(signal, libc::SIG_DFL);
            libc::raise(signal);
        });

        SIGNAL_SEND = send.into_raw_fd();
        libc::signal(SIGINT, handler as libc::sighandler_t);
        libc::signal(SIGTERM, handler as libc::sighandler_t);
        Ok(())
    }

    // Invoked on a background thread. Process exits after this returns.
    fn on_signal(guards: &mut GuardTable) {
        for guard in guards.drain() {
            guard();
        }
    }

    unsafe extern "C" fn handler(signal: c_int) {
        // Treat the second signal as instantly fatal.
        static SIGNALED: AtomicBool = AtomicBool::new(false);
        if SIGNALED.swap(true, Ordering::Relaxed) {
            libc::signal(signal, libc::SIG_DFL);
            libc::raise(signal);
        }

        let buf = [signal as u8];
        libc::write(SIGNAL_SEND, buf.as_ptr().cast(), buf.len());
    }

    static mut SIGNAL_SEND: RawFd = 0;
}

#[cfg(not(unix))]
mod platform {
    use super::*;

    /// Safety: this function is safe to call, but is marked as unsafe to have
    /// the same signature as other `init` functions in other platforms.
    pub unsafe fn init() -> io::Result<()> {
        Ok(())
    }
}
