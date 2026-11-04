//! Process-global tracing-subscriber installation.
//!
//! Installing a subscriber is a process-wide side effect, not a DI-resolved value,
//! so this is a main-side helper rather than an injectable. It is gated behind the
//! `tracing-subscriber` feature so the dependency is only pulled in when a binary
//! actually drives logging setup.

use std::ffi::OsStr;

use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry, fmt};

use crate::builtins::config::{LogFormat, LoggingConfig, SpanEvents};
use crate::logs::ScopeLogLayer;

/// A type-erased subscriber layer that can be composed onto the framework subscriber, e.g.
/// per-run job log capture. Boxed so callers in other crates can hand in a layer the
/// (protocol-agnostic) app core has no knowledge of.
pub type BoxedLayer = Box<dyn Layer<Registry> + Send + Sync + 'static>;

/// Errors raised while installing the global tracing subscriber from a
/// [`LoggingConfig`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum InitTracingError {
    /// The configured `level` directive could not be parsed as an `EnvFilter`.
    #[error("invalid log level filter '{filter}': {source}")]
    Filter {
        filter: String,
        #[source]
        source: tracing_subscriber::filter::ParseError,
    },

    /// A global subscriber was already installed for this process.
    #[error("a global tracing subscriber is already installed")]
    AlreadyInstalled,
}

/// Installs the process-global tracing subscriber from `config`.
///
/// Builds an `EnvFilter` from the configured level directive and selects the
/// `fmt` output format. Returns [`InitTracingError::AlreadyInstalled`] if a global
/// subscriber is already in place, so callers can decide whether that is fatal.
pub fn init_tracing(config: &LoggingConfig) -> Result<(), InitTracingError> {
    init_tracing_with_layers(config, Vec::new())
}

#[cfg(feature = "cli")]
pub(crate) fn init_tracing_resolved(
    config: &LoggingConfig,
    extra: Vec<BoxedLayer>,
) -> Result<(), InitTracingError> {
    let filter = EnvFilter::try_new(&config.level).map_err(|source| InitTracingError::Filter {
        filter: config.level.clone(),
        source,
    })?;
    let mut layers = vec![fmt_layer(config)?.with_filter(filter).boxed()];

    layers.push(scope_log_layer(config, None)?);
    layers.extend(extra);

    Registry::default()
        .with(layers)
        .try_init()
        .map_err(|_| InitTracingError::AlreadyInstalled)
}

/// Installs the process-global subscriber from `config`, composing `extra` layers on top of the
/// framework's filtered `fmt` layer.
///
/// The escape hatch for crates above the app core that need to add their own capture — e.g.
/// `upwell-jobs` layering per-run log capture — without the app core depending on them.
/// Layers must be composed before installation, so an already-installed subscriber cannot be
/// extended after the fact; call this once, at startup.
pub fn init_tracing_with_layers(
    config: &LoggingConfig,
    extra: Vec<BoxedLayer>,
) -> Result<(), InitTracingError> {
    let rust_log = std::env::var_os(EnvFilter::DEFAULT_ENV);
    let filter = env_filter(config, rust_log.as_deref())?;

    // Filter only console formatting. Extension layers enforce their own capture policies and may
    // intentionally observe events that are more verbose than terminal output.
    let mut layers: Vec<BoxedLayer> = vec![fmt_layer(config)?.with_filter(filter).boxed()];

    layers.push(scope_log_layer(config, rust_log.as_deref())?);
    layers.extend(extra);

    Registry::default()
        .with(layers)
        .try_init()
        .map_err(|_| InitTracingError::AlreadyInstalled)
}

/// The scope log capture layer, behind the same filter as the console output so capture
/// defaults to the level the subscriber logs at.
fn scope_log_layer(
    config: &LoggingConfig,
    rust_log: Option<&OsStr>,
) -> Result<BoxedLayer, InitTracingError> {
    let filter = env_filter(config, rust_log)?;

    Ok(ScopeLogLayer::new().with_filter(filter).boxed())
}

fn env_filter(
    config: &LoggingConfig,
    rust_log: Option<&OsStr>,
) -> Result<EnvFilter, InitTracingError> {
    let filter = rust_log
        .and_then(OsStr::to_str)
        .unwrap_or(config.level.as_str());

    EnvFilter::try_new(filter).map_err(|source| InitTracingError::Filter {
        filter: filter.to_owned(),
        source,
    })
}

/// Builds the boxed `fmt` layer for the configured output format, honouring the ansi setting.
fn fmt_layer(config: &LoggingConfig) -> Result<BoxedLayer, InitTracingError> {
    let base = fmt::layer()
        .with_ansi(config.ansi)
        .with_span_events(fmt_span(config.span_events))
        .with_target(config.target)
        .with_level(config.level_display)
        .with_thread_ids(config.thread_ids)
        .with_thread_names(config.thread_names)
        .with_file(config.file)
        .with_line_number(config.line_number);

    let layer = match config.format {
        LogFormat::Full => base.boxed(),
        LogFormat::Compact => base.compact().boxed(),
        LogFormat::Pretty => base.pretty().boxed(),
        LogFormat::Json => base
            .json()
            .flatten_event(config.flatten_event)
            .with_current_span(config.current_span)
            .with_span_list(config.current_span)
            .boxed(),
    };

    Ok(layer)
}

fn fmt_span(events: SpanEvents) -> FmtSpan {
    match events {
        SpanEvents::None => FmtSpan::NONE,
        SpanEvents::New => FmtSpan::NEW,
        SpanEvents::Enter => FmtSpan::ENTER,
        SpanEvents::Exit => FmtSpan::EXIT,
        SpanEvents::Close => FmtSpan::CLOSE,
        SpanEvents::Active => FmtSpan::ACTIVE,
        SpanEvents::Full => FmtSpan::FULL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::config::LoggingConfig;

    #[test]
    fn rust_log_overrides_configured_level_and_targets() {
        let config = LoggingConfig {
            level: "invalid[".to_owned(),
            ..LoggingConfig::default()
        };

        let result = env_filter(&config, Some(OsStr::new("warn,upwell=trace")));

        assert!(result.is_ok());
    }

    #[test]
    fn configured_level_is_used_without_rust_log() {
        let config = LoggingConfig {
            level: "invalid[".to_owned(),
            ..LoggingConfig::default()
        };

        let result = env_filter(&config, None);

        assert!(matches!(result, Err(InitTracingError::Filter { .. })));
    }
}
