//! The layer that routes events to the scope instance they were made in.

use std::sync::Arc;

use tracing::span::Attributes;
use tracing::{Event, Id, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

use super::sink::LayerId;
use super::span as handoff;
use super::{SCOPE_SPAN_NAME, ScopeLogSink, entered, route, sink};

/// Routes every event made inside a capturing scope instance to that instance's sink.
///
/// The innermost `upwell.scope` span around an event that has a sink names the candidate, so
/// parallel instances never see each other's records and an instance nested in another
/// delivers only to itself. The invocation context the event is made in decides whether the
/// candidate applies: inside an invocation, an event reaches only an instance on the
/// invocation's scope chain or one opening under it, else the innermost capturing instance of
/// the invocation, so work carried with Upwell's propagation primitives logs to the invocation
/// it was captured in. Work captured outside every invocation reaches no instance. Code that
/// never runs inside an invocation context is routed by the span alone. Events made outside
/// any capturing instance reach only the other layers: uncaptured, never misrouted. Capture
/// tees: the layer never stops an event reaching the rest of the subscriber.
///
/// A span entered by hand inside an invocation's context that does not run in its instance
/// therefore does not capture into that instance, even one that opened under the invocation's
/// innermost one: only while an instance still opens do its events reach it that way.
///
/// `init_tracing` installs it behind the same filter as the console output, so capture defaults
/// to the level the subscriber logs at. When composing it into your own subscriber, wrap it in
/// the filter that should bound capture, and keep that filter admitting the `upwell::scope`
/// target at error level: a span the layer never sees cannot hold a sink. A run's capture
/// level narrows what that filter admits; it can only widen capture as far as the filter lets
/// events through, because enabling a callsite for one layer enables it for every layer.
///
/// Inside an invocation, a layer falls back only to instances whose routing span it took
/// itself: events made under another subscriber, or one whose filter turns `upwell::scope`
/// off, never enter a capture they could not see.
#[derive(Clone, Copy, Debug)]
pub struct ScopeLogLayer {
    id: LayerId,
}

impl ScopeLogLayer {
    /// The layer.
    pub const fn new() -> Self {
        Self {
            id: LayerId::UNREGISTERED,
        }
    }

    /// The identity the sinks this layer takes record.
    pub(super) fn id(&self) -> LayerId {
        self.id
    }
}

impl Default for ScopeLogLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Layer<S> for ScopeLogLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_layer(&mut self, _subscriber: &mut S) {
        self.id = LayerId::register();
    }

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

        sink.attach(self.id);
        span.extensions_mut().insert(sink);
    }

    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        if !sink::any_open() {
            return;
        }

        let Some(span) = ctx.span(id) else {
            return;
        };
        let found = span
            .scope()
            .find_map(|span| span.extensions().get::<Arc<ScopeLogSink>>().cloned());

        entered::enter(self.id, id, found);
    }

    fn on_exit(&self, id: &Id, _ctx: Context<'_, S>) {
        drop(entered::exit(self.id, id));
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        if !sink::any_open() {
            return;
        }

        let span_sink = ctx.event_scope(event).and_then(|scope| {
            scope
                .into_iter()
                .find_map(|span| span.extensions().get::<Arc<ScopeLogSink>>().cloned())
        });

        if let Some(sink) = route::route(span_sink, |sink| sink.is_attached_to(self.id)) {
            sink.deliver(event);
        }
    }
}
