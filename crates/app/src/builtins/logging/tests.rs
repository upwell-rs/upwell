use super::*;
use crate::builtins::config::LoggingConfig;
use crate::logs::{InstanceLogs, LogOptions, ScopeLog};
use futures::StreamExt;
use upwell_core::{Scope, ScopeId};

/// A scope used to exercise the installed capture filter.
struct TestScope;

impl Scope for TestScope {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/log-filter");
    const RANK: u8 = 1;
    const NAME: &'static str = "TestScope";
}

/// Opens a capturing instance under the configured filter and returns captured messages.
fn captured_messages(directives: &str) -> (bool, Vec<String>) {
    let config = LoggingConfig::default();
    let layer = scope_log_layer(&config, Some(OsStr::new(directives)))
        .expect("filter directives are valid");
    let subscriber = Registry::default().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);
    let _guard = tracing::dispatcher::set_default(&dispatch);
    let options = LogOptions {
        capture: true,
        ..LogOptions::default()
    };
    let logs = InstanceLogs::open(&TestScope, &options, None);
    let sink = logs.sink().expect("capture is enabled");
    let attached = sink.is_attached();
    let stream = sink.subscribe();

    {
        let _entered = logs.span().enter();

        tracing::info!(target: "myapp", "application event");
        tracing::error!(target: "upwell::scope", "framework event");
    }

    logs.finish();

    let messages = futures::executor::block_on(
        stream
            .filter_map(|item| async move {
                match item {
                    ScopeLog::Record(record) => Some(record.message),
                    ScopeLog::Lagged(_) => None,
                }
            })
            .collect(),
    );

    (attached, messages)
}

#[test]
fn routing_span_does_not_enable_other_events() {
    let (attached, messages) = captured_messages("myapp=info");

    assert!(attached);
    assert_eq!(messages, ["application event"]);
}

#[test]
fn explicit_scope_off_disables_capture() {
    let (attached, messages) = captured_messages("myapp=info,upwell::scope=off");

    assert!(!attached);
    assert!(messages.is_empty());
}

#[test]
fn target_prefix_does_not_count_as_scope_target() {
    let (attached, messages) = captured_messages("myapp=info,upwell::scope_extra=off");

    assert!(attached);
    assert_eq!(messages, ["application event"]);
}

#[test]
fn later_identical_directive_overrides_earlier_one() {
    let (attached, messages) =
        captured_messages("myapp=info,upwell::scope=off,upwell::scope=error");

    assert!(attached);
    assert_eq!(messages, ["application event", "framework event"]);
}

#[test]
fn field_name_directive_follows_env_filter_span_matching() {
    let (attached, messages) = captured_messages("myapp=info,upwell::scope[{other}]=off");

    assert!(!attached);
    assert!(messages.is_empty());
}

#[test]
fn span_specific_off_preserves_static_fallback() {
    let (attached, messages) = captured_messages("myapp=info,upwell::scope[unrelated]=off");

    assert!(attached);
    assert_eq!(messages, ["application event"]);
}

#[test]
fn dynamic_span_off_does_not_override_static_fallback() {
    let (attached, messages) = captured_messages("myapp=info,upwell::scope[upwell.scope]=off");

    assert!(attached);
    assert_eq!(messages, ["application event"]);
}

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
