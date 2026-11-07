//! Which instance's sink an event belongs to, decided by the invocation context.
//!
//! The `upwell.scope` span an event is made in names a candidate sink; the invocation context
//! the event is made in decides whether it applies:
//!
//! - with no context on the task or thread at all, the span decides, as for code that never
//!   runs inside the framework's propagation;
//! - with a masked context, for work captured outside every invocation, no sink applies;
//! - with an entered context, the span's sink applies when its instance is on the context's
//!   scope chain or opens under the context's innermost instance, as an instance does while
//!   it opens. Otherwise the innermost capturing instance on the context's chain applies.
//!
//! A sink whose instance finished no longer applies.

use std::sync::Arc;

use upwell_di::ScopeContainer;
use upwell_di::context::{ContextState, ScopeChain, with_context_state};

use super::ScopeLogSink;
use super::sink::any_open;

/// The sink an event made here belongs to, given `span_sink`, the sink of the innermost
/// capturing `upwell.scope` span around it.
pub(super) fn route(span_sink: Option<Arc<ScopeLogSink>>) -> Option<Arc<ScopeLogSink>> {
    if !any_open() {
        return None;
    }

    let span_sink = span_sink.filter(|sink| sink.is_open());
    let parent = span_sink.as_ref().and_then(|sink| sink.parent());
    let routed = with_context_state(|state| match state {
        ContextState::Absent => span_sink,
        ContextState::Masked => None,
        ContextState::Entered(chain) => entered(span_sink, parent.as_ref(), chain),
    });

    drop(parent);

    routed
}

/// Hands `sink` to `container`, the capturing instance it belongs to, so work running in an
/// invocation context inside the instance finds it. The instance releases it with its last
/// handle. Only a scope instance a builder opened retains a sink, never a root, whose slot
/// holds its generation's guard.
pub(crate) fn retain_sink(container: &ScopeContainer, sink: Option<&Arc<ScopeLogSink>>) {
    let Some(sink) = sink else {
        return;
    };
    let retained = container.retain_drop_guard(Arc::clone(sink));

    // Ignored in release builds: a builder retains a sink once, right after it opened the
    // instance, so a second one cannot occur; the instance would keep the first.
    debug_assert!(retained.is_ok(), "a scope instance retains one log sink");
}

/// The sink an event belongs to inside an entered context whose scope chain is `chain`.
fn entered(
    span_sink: Option<Arc<ScopeLogSink>>,
    parent: Option<&Arc<ScopeContainer>>,
    chain: ScopeChain<'_>,
) -> Option<Arc<ScopeLogSink>> {
    match span_sink {
        Some(sink) if belongs(&sink, parent, chain) => Some(sink),
        Some(_) | None => chain.into_iter().find_map(retained).map(Arc::clone),
    }
}

/// Whether `sink`'s instance, opened under `parent`, is on `chain` or opens under its
/// innermost instance.
fn belongs(
    sink: &Arc<ScopeLogSink>,
    parent: Option<&Arc<ScopeContainer>>,
    chain: ScopeChain<'_>,
) -> bool {
    let on_chain = chain
        .into_iter()
        .any(|instance| retained(instance).is_some_and(|held| Arc::ptr_eq(held, sink)));

    on_chain || chain.innermost().zip(parent).is_some_and(descends)
}

/// Whether an instance opened under `parent` descends from `ancestor`.
fn descends((ancestor, parent): (&Arc<ScopeContainer>, &Arc<ScopeContainer>)) -> bool {
    ScopeChain::starting_at(parent).any(|instance| Arc::ptr_eq(instance, ancestor))
}

/// The sink `instance` retains, while it captures.
fn retained(instance: &Arc<ScopeContainer>) -> Option<&Arc<ScopeLogSink>> {
    instance
        .drop_guard::<Arc<ScopeLogSink>>()
        .filter(|sink| sink.is_open())
}
