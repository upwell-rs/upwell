use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::{Layer, Registry};

#[derive(Clone, Debug)]
pub(crate) struct CapturedEvent {
    pub target: String,
    pub fields: BTreeMap<String, String>,
}

#[derive(Clone, Default)]
struct CaptureLayer {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl<S: Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);

        self.events
            .lock()
            .expect("capture lock is not poisoned")
            .push(CapturedEvent {
                target: event.metadata().target().to_string(),
                fields: visitor.fields,
            });
    }
}

#[derive(Default)]
struct FieldVisitor {
    fields: BTreeMap<String, String>,
}

impl Visit for FieldVisitor {
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.fields
            .insert(field.name().to_string(), format!("{value:?}"));
    }
}

pub(crate) fn capture_events(run: impl FnOnce()) -> Vec<CapturedEvent> {
    let layer = CaptureLayer::default();
    let events = Arc::clone(&layer.events);
    let subscriber = Registry::default().with(layer);

    tracing::subscriber::with_default(subscriber, run);

    Arc::try_unwrap(events)
        .expect("capture layer released")
        .into_inner()
        .expect("capture lock is not poisoned")
}
