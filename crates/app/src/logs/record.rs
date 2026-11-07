//! The owned items a scope log stream yields.

use std::sync::Arc;
use std::time::SystemTime;

use tracing::field::{Field, Visit};
use tracing::{Event, Level};
use upwell_core::ScopeId;

use super::LogField;

/// One event captured inside a scope instance, owned so it outlives the event.
#[derive(Clone, Debug)]
pub struct LogRecord {
    /// The event's severity. Ordered like `tracing`: more verbose is greater.
    pub level: Level,
    /// The event's tracing target.
    pub target: String,
    /// The event's `message` field.
    pub message: String,
    /// The event's other fields.
    pub fields: Vec<LogField>,
    /// When the event was captured.
    pub timestamp: SystemTime,
    /// The scope the instance belongs to.
    pub scope: ScopeId,
    /// The instance's loggable parameters, shared by every record of the instance.
    pub params: Arc<[LogField]>,
}

/// One item of a scope log stream.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ScopeLog {
    /// A captured event.
    Record(LogRecord),
    /// The subscriber's buffer overflowed and this many records were dropped, after the
    /// records the stream already yielded.
    Lagged(usize),
}

impl LogRecord {
    /// The record of `event`, made in an instance of `scope` with `params`.
    pub(super) fn from_event(event: &Event<'_>, scope: ScopeId, params: Arc<[LogField]>) -> Self {
        let mut visitor = EventVisitor::default();

        event.record(&mut visitor);

        Self {
            level: *event.metadata().level(),
            target: event.metadata().target().to_owned(),
            message: visitor.message,
            fields: visitor.fields,
            timestamp: SystemTime::now(),
            scope,
            params,
        }
    }
}

/// Splits an event's fields into its message and the rest.
#[derive(Default)]
struct EventVisitor {
    message: String,
    fields: Vec<LogField>,
}

impl EventVisitor {
    fn push(&mut self, field: &Field, value: String) {
        if field.name() == "message" {
            self.message = value;
        } else {
            self.fields.push(LogField::new(field.name(), value));
        }
    }
}

impl Visit for EventVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.push(field, value.to_owned());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.push(field, format!("{value:?}"));
    }
}
