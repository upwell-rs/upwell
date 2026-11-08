//! Per-instance scope logs: an `upwell.scope` span for every instance, a [`ScopeLogLayer`] that
//! routes the events made inside it to the instance, and the [`ScopeLogStream`] a run exposes.
//!
//! The span names the instance an event was made in, which covers every event, those of
//! third-party crates included; the invocation context decides whether that instance applies,
//! so work carried into another task, thread or pool logs to the invocation it belongs to. The logs read the context; propagation knows nothing about logs. Capture is off
//! unless it is asked for: while no instance captures, an event costs one atomic read.
//!
//! # Seam for the end-of-scope hook
//!
//! An instance's work ends in [`InstanceLogs::finish`], which closes the sink and so ends every
//! stream. An end-of-scope hook runs after the instance's work and immediately before that
//! call, so the events it makes are still captured.

mod current;
#[cfg(feature = "tracing-subscriber")]
mod entered;
mod field;
#[cfg(feature = "tracing-subscriber")]
mod layer;
mod options;
#[cfg(feature = "tracing-subscriber")]
mod reach;
#[cfg_attr(not(feature = "tracing-subscriber"), allow(dead_code))]
mod record;
mod render;
mod route;
#[cfg_attr(not(feature = "tracing-subscriber"), allow(dead_code))]
mod sink;
mod span;
#[cfg_attr(not(feature = "tracing-subscriber"), allow(dead_code))]
mod stream;

pub use current::ScopeLogs;
pub use field::LogField;
#[cfg(feature = "tracing-subscriber")]
pub use layer::ScopeLogLayer;
pub use options::{DEFAULT_LOG_BUFFER, LogOptions};
pub use record::{LogRecord, ScopeLog};
pub use sink::ScopeLogSink;
pub use span::InstanceLogs;
pub use stream::ScopeLogStream;

pub(crate) use route::retain_sink;
pub(crate) use span::{deferred_sink, describe_sink};

/// The name of the span every scope instance runs in.
pub const SCOPE_SPAN_NAME: &str = "upwell.scope";

/// The tracing target of the scope span and of the framework's own scope log lines.
pub const SCOPE_LOG_TARGET: &str = "upwell::scope";

#[cfg(test)]
mod tests;
