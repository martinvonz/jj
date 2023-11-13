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
** [ BSD3 license: http://www.opensource.org/licenses/mit-license.php ]
*/

#ifndef _LIBFAULT_H_
#define _LIBFAULT_H_

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*libfault_custom_diagnostics)(void* data);

/**
 * libfault_init():
 *
 * Initializes the libfault library. Must be called before any other
 * functions.
 *
 * - Returns nothing.
 */
void libfault_init(void);

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
void libfault_set_app_name(const char* name);

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
void libfault_set_app_version(const char* version);

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
void libfault_set_log_name(const char* path);

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
void libfault_set_bugreport_url(const char* url);

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
void libfault_set_custom_diagnostics(libfault_custom_diagnostics callback);

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
void libfault_set_custom_diagnostics_data(void* data);

/**
 * libfault_install_handlers():
 *
 * Install default libfault signal handlers for crash reporting.
 *
 * This can only be called after ${libfault_init}.
 *
 * - Returns nothing.
 */
void libfault_install_handlers(void);

#ifdef __cplusplus
} // extern "C"
#endif

#endif /* !_LIBFAULT_H_ */
