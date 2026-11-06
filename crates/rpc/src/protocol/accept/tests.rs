use std::io::{Error, ErrorKind};

use super::is_transient_accept_error;

#[test]
fn retries_only_plausibly_transient_accept_error_kinds() {
    for kind in [
        ErrorKind::Interrupted,
        ErrorKind::WouldBlock,
        ErrorKind::ConnectionAborted,
        ErrorKind::ConnectionReset,
        ErrorKind::TimedOut,
        ErrorKind::OutOfMemory,
    ] {
        assert!(is_transient_accept_error(&Error::from(kind)), "{kind:?}");
    }

    for kind in [
        ErrorKind::PermissionDenied,
        ErrorKind::InvalidInput,
        ErrorKind::AddrNotAvailable,
        ErrorKind::Unsupported,
        ErrorKind::Other,
    ] {
        assert!(!is_transient_accept_error(&Error::from(kind)), "{kind:?}");
    }
}

#[cfg(unix)]
#[test]
fn exhaustion_errnos_are_retried_with_the_targets_own_numbering() {
    for raw in [libc::ENOMEM, libc::ENFILE, libc::EMFILE] {
        assert!(
            is_transient_accept_error(&Error::from_raw_os_error(raw)),
            "{raw}"
        );
    }
}

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
#[test]
fn buffer_exhaustion_is_retried_where_the_target_reports_it() {
    assert!(is_transient_accept_error(&Error::from_raw_os_error(
        libc::ENOBUFS
    )));
}

#[cfg(unix)]
#[test]
fn unrelated_errnos_are_not_retried() {
    for raw in [libc::EACCES, libc::EINVAL, libc::EBADF, libc::EPERM] {
        assert!(
            !is_transient_accept_error(&Error::from_raw_os_error(raw)),
            "{raw}"
        );
    }
}

#[cfg(windows)]
#[test]
fn windows_exhaustion_codes_are_retried_and_unix_numbers_are_not() {
    for raw in [4, 8, 14, 10024, 10055] {
        assert!(is_transient_accept_error(&Error::from_raw_os_error(raw)));
    }

    for raw in [12, 23, 24, 105] {
        assert!(!is_transient_accept_error(&Error::from_raw_os_error(raw)));
    }
}
