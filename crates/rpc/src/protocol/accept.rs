//! Classifying accept failures the serve loop retries.

pub(super) fn is_transient_accept_error(error: &std::io::Error) -> bool {
    if matches!(
        error.kind(),
        std::io::ErrorKind::Interrupted
            | std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::TimedOut
    ) {
        return true;
    }

    // Resource exhaustion reported by accept(2) / Winsock is generally exposed only as a
    // platform raw code. Keep the tables target-specific: small Windows system-error values
    // overlap unrelated Unix errno values (and vice versa).
    is_transient_accept_os_error(error.raw_os_error())
}

#[cfg(windows)]
fn is_transient_accept_os_error(raw: Option<i32>) -> bool {
    matches!(
        raw,
        Some(
            4      // ERROR_TOO_MANY_OPEN_FILES
            | 8    // ERROR_NOT_ENOUGH_MEMORY
            | 14   // ERROR_OUTOFMEMORY
            | 10024 // WSAEMFILE
            | 10055 // WSAENOBUFS
        )
    )
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn is_transient_accept_os_error(raw: Option<i32>) -> bool {
    matches!(raw, Some(12 | 23 | 24 | 105)) // ENOMEM, ENFILE, EMFILE, ENOBUFS
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
fn is_transient_accept_os_error(raw: Option<i32>) -> bool {
    matches!(raw, Some(12 | 23 | 24 | 55)) // ENOMEM, ENFILE, EMFILE, ENOBUFS
}

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
fn is_transient_accept_os_error(raw: Option<i32>) -> bool {
    matches!(raw, Some(12 | 23 | 24)) // ENOMEM, ENFILE, EMFILE
}

#[cfg(not(any(unix, windows)))]
fn is_transient_accept_os_error(_raw: Option<i32>) -> bool {
    false
}
