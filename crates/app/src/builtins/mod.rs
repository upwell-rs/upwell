//! Framework-provided builtin injectables, config structs, and helpers.
//!
//! v1 is minimal infra-only:
//! - the seeded [`ShutdownHandle`](crate::lifecycle::ShutdownHandle) singleton
//!   injectable (impls live in [`shutdown`]),
//! - the seeded [`RuntimeReloader`] singleton injectable (in [`reloader`]),
//! - opt-in config property structs [`ServerConfig`] / [`LoggingConfig`],
//! - the feature-gated [`init_tracing`] subscriber helper.

pub mod config;
pub mod reloader;
pub mod shutdown;

#[cfg(feature = "tracing-subscriber")]
pub mod logging;

pub use config::{LogFormat, LoggingConfig, ParseLogFormatError, ServerConfig, SpanEvents};
pub use reloader::RuntimeReloader;

#[cfg(feature = "tracing-subscriber")]
pub use logging::{BoxedLayer, InitTracingError, init_tracing, init_tracing_with_layers};

#[cfg(test)]
mod tests;
