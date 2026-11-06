//! Classifying accept failures the serve loop retries.

mod os;

#[cfg(test)]
mod tests;

pub(super) fn is_transient_accept_error(error: &std::io::Error) -> bool {
    if matches!(
        error.kind(),
        std::io::ErrorKind::Interrupted
            | std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::OutOfMemory
    ) {
        return true;
    }

    error
        .raw_os_error()
        .is_some_and(|raw| os::TRANSIENT.contains(&raw))
}
