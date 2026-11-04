//! The layer that routes events to the scope instance they were made in.

use std::sync::Arc;

use tracing::span::Attributes;
use tracing::{Event, Id, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

use super::span as handoff;
use super::{SCOPE_SPAN_NAME, ScopeLogSink};

/// Routes every event made inside a capturing scope instance to that instance's sink.
///
/// It walks the event's span stack to the innermost `upwell.scope` span that has a sink, so
/// parallel instances never see each other's records and an instance nested in another
/// delivers only to itself. Events made outside any capturing instance, or on a task that lost
/// the span, reach only the other layers: uncaptured, never misrouted. Capture tees: the layer
/// never stops an event reaching the rest of the subscriber.
///
/// `init_tracing` installs it behind the same filter as the console output, so capture defaults
/// to the level the subscriber logs at. When composing it into your own subscriber, wrap it in
/// the filter that should bound capture, and keep that filter admitting the `upwell::scope`
/// target at error level: a span the layer never sees cannot hold a sink. A run's capture
/// level narrows what that filter admits; it can only widen capture as far as the filter lets
/// events through, because enabling a callsite for one layer enables it for every layer.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScopeLogLayer;

impl ScopeLogLayer {
    /// The layer.
    pub const fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for ScopeLogLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if attrs.metadata().name() != SCOPE_SPAN_NAME {
            return;
        }

        let Some(sink) = handoff::take_handoff() else {
            return;
        };
        let Some(span) = ctx.span(id) else {
            return;
        };

        sink.attach();
        span.extensions_mut().insert(sink);
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let Some(scope) = ctx.event_scope(event) else {
            return;
        };

        for span in scope {
            if let Some(sink) = span.extensions().get::<Arc<ScopeLogSink>>() {
                sink.deliver(event);

                return;
            }
        }
    }
}
