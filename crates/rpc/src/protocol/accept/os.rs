//! The raw OS error codes of accept failures that signal temporary resource exhaustion.
//!
//! `std::io::ErrorKind` maps out-of-memory but not file-table or buffer exhaustion. On Unix the
//! codes come from `libc`, which knows each target's numbering (`ENOBUFS` differs between
//! kernels and architectures). The Windows values overlap unrelated Unix errno values, so each
//! family keeps its own table.

#[cfg(all(
    unix,
    any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    )
))]
pub(super) const TRANSIENT: &[i32] = &[libc::ENOMEM, libc::ENFILE, libc::EMFILE, libc::ENOBUFS];

#[cfg(all(
    unix,
    not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))
))]
pub(super) const TRANSIENT: &[i32] = &[libc::ENOMEM, libc::ENFILE, libc::EMFILE];

#[cfg(windows)]
pub(super) const TRANSIENT: &[i32] = &[4, 8, 14, 10024, 10055];

#[cfg(not(any(unix, windows)))]
pub(super) const TRANSIENT: &[i32] = &[];
