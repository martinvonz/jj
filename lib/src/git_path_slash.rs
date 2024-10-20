//! This is from <https://github.com/rhysd/path-slash>, but tweaked to
//! change any verbatim prefix to non-verbatim, as git does.
//!
//! This is MIT licensed. See lib/LICENSE for a copy of the license.

use std::borrow::Cow;
use std::path::Path;

#[cfg(target_os = "windows")]
mod windows {
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::MAIN_SEPARATOR;

    use super::*;

    // Workaround for Windows. There is no way to extract raw byte sequence from
    // `OsStr` (in `Path`). And `OsStr::to_string_lossy` may cause extra
    // heap allocation.
    pub(crate) fn ends_with_main_sep(p: &Path) -> bool {
        p.as_os_str().encode_wide().last() == Some(MAIN_SEPARATOR as u16)
    }
}

/// On non-Windows targets, this is just [Path::to_string_lossy].
///
/// On Windows targets, this converts backslashes to forward slashes,
/// and removes the `\\?\` verbatim prefix.
#[cfg(not(target_os = "windows"))]
pub fn to_slash_lossy(path: &Path) -> Cow<'_, str> {
    path.to_string_lossy()
}

/// On non-Windows targets, this is just [Path::to_string_lossy].
///
/// On Windows targets, this converts backslashes to forward slashes,
/// and removes the `\\?\` verbatim prefix.
#[cfg(target_os = "windows")]
pub fn to_slash_lossy(path: &Path) -> Cow<'_, str> {
    use std::path::Component;
    use std::path::Prefix;

    let mut buf = String::new();
    for c in path.components() {
        match c {
            Component::RootDir => { /* empty */ }
            Component::CurDir => buf.push('.'),
            Component::ParentDir => buf.push_str(".."),
            Component::Prefix(prefix_component) => {
                match prefix_component.kind() {
                    Prefix::Disk(disk) | Prefix::VerbatimDisk(disk) => {
                        if let Some(c) = char::from_u32(disk as u32) {
                            buf.push(c);
                            buf.push(':');
                        }
                    }
                    Prefix::UNC(host, share) | Prefix::VerbatimUNC(host, share) => {
                        // Write it as non-verbatim but with two forward slashes // instead of
                        // \\. I think this sounds right? https://learn.microsoft.com/en-us/dotnet/core/compatibility/core-libraries/5.0/unc-path-recognition-unix
                        buf.push_str("//");
                        buf.push_str(&host.to_string_lossy());
                        buf.push_str("/");
                        buf.push_str(&share.to_string_lossy());
                    }
                    // Just ignore it and hope for the best?
                    Prefix::Verbatim(_) => {}
                    Prefix::DeviceNS(_) => {}
                }
                // C:\foo is [Prefix, RootDir, Normal]. Avoid C://
                continue;
            }
            Component::Normal(s) => buf.push_str(&s.to_string_lossy()),
        }
        buf.push('/');
    }

    if !windows::ends_with_main_sep(path) && buf != "/" && buf.ends_with('/') {
        buf.pop(); // Pop last '/'
    }

    Cow::Owned(buf)
}
