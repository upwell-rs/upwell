use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::{Layer, Registry};

/// One captured tracing event: its target, message, and every recorded field rendered
/// as text.
#[derive(Clone, Debug)]
pub(crate) struct CapturedEvent {
    pub target: String,
    pub fields: BTreeMap<String, String>,
}

impl CapturedEvent {
    /// The rendered value of one field.
    pub(crate) fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }
}

/// A layer that records every event into a shared buffer.
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

/// Renders every field of one event as text.
#[derive(Default)]
struct FieldVisitor {
    fields: BTreeMap<String, String>,
}

impl Visit for FieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.fields
            .insert(field.name().to_string(), format!("{value:?}"));
    }
}

/// Runs `future` to completion with a capturing subscriber installed as the thread's
/// default, and returns its output plus every event it emitted.
///
/// The subscriber is thread-local, so the future must not migrate threads: use it from
/// a current-thread runtime such as the default `#[tokio::test]`.
pub(crate) async fn capture_events<T>(future: impl Future<Output = T>) -> (T, Vec<CapturedEvent>) {
    let layer = CaptureLayer::default();
    let events = Arc::clone(&layer.events);
    let subscriber = Registry::default().with(layer);
    let guard = tracing::subscriber::set_default(subscriber);

    let output = future.await;

    drop(guard);

    let events = events.lock().expect("capture lock is not poisoned").clone();

    (output, events)
}
