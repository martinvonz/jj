// Copyright 2020 The Jujutsu Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Hard-crash reporting library.

#[allow(unused_macros)]
use core::ffi::c_char;

// Full set of external C APIs. These should under no circumstances ever be made
// `pub`.
#[cfg(not(target_os = "windows"))]
extern "C" {
    fn libfault_init();
    fn libfault_install_handlers();

    fn libfault_set_app_name(name: *const c_char);
    fn libfault_set_app_version(version: *const c_char);
    fn libfault_set_log_name(name: *const c_char);
    fn libfault_set_bugreport_url(url: *const c_char);
}

/// Information about the application that is running. In the event of a crash,
/// this information will be included in the resulting crash report.
///
/// Do **NOT** construct this directly. Instead, use the [crash_handler_info!]
/// to create a global variable with `static` lifetime.
pub struct AppInfo {
    pub app_name: &'static str,
    pub app_version: &'static str,
    pub log_name: &'static str,
    pub bugreport_url: &'static str,
}

/// Initialize the in-process crash reporting library with the given application
/// information.
pub fn install(info: &'static AppInfo) {
    #[cfg(not(target_os = "windows"))]
    unsafe {
        libfault_init();
        libfault_set_app_name(info.app_name.as_ptr() as *const c_char);
        libfault_set_app_version(info.app_version.as_ptr() as *const c_char);
        libfault_set_log_name(info.log_name.as_ptr() as *const c_char);
        libfault_set_bugreport_url(info.bugreport_url.as_ptr() as *const c_char);
        libfault_install_handlers();
    }
}

/// Generate an [AppInfo]. It is required for this to be used for the argument
/// to [install].
#[macro_export]
macro_rules! crash_handler_info {
    ($($key:ident: $value:expr),* $(,)?) => {
        // XXX: every AppInfo field has to have a \0 put on the end in order for
        // moving across the FFI to work correctly; otherwise logs will try to
        // print strings without null termination, and you can have a double
        // crash in the child process.
        //
        // FIXME (aseipp): redesign the libfault interface to take (ptr, len)
        // pairs like ordinary rust strings.
        jj_cbits::libfault::AppInfo {
            $($key: concat!($value, "\0"),)*
        }
    };
}
