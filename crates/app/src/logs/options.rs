//! How an instance captures its logs.

use tracing::level_filters::LevelFilter;

/// The records a subscriber's buffer holds unless [`LogOptions::buffer`] says otherwise.
pub const DEFAULT_LOG_BUFFER: usize = 1024;

/// How one scope instance captures its logs, set on its builder.
///
/// Capture is off by default. Asking for it, with `capture_logs` or by taking a stream from a
/// run, creates the instance's sink; an instance without one adds no per-event work.
#[derive(Clone, Copy, Debug)]
pub struct LogOptions {
    /// Whether the instance captures logs.
    pub capture: bool,
    /// The capture level, or `None` to take whatever the subscriber's filter admits. A level
    /// stricter than the filter narrows capture; a more verbose one only widens it as far as
    /// the filter on the [`ScopeLogLayer`](super::ScopeLogLayer) lets events through.
    pub level: Option<LevelFilter>,
    /// The records each subscriber's buffer holds before older-than-newest ones are dropped.
    pub buffer: usize,
}

impl LogOptions {
    /// Capture off, the default level and buffer.
    pub const fn new() -> Self {
        Self {
            capture: false,
            level: None,
            buffer: DEFAULT_LOG_BUFFER,
        }
    }
}

impl Default for LogOptions {
    fn default() -> Self {
        Self::new()
    }
}
