//! Which instance's sink an event belongs to, decided by the invocation context.
//!
//! The `upwell.scope` span an event is made in names a candidate sink; the invocation context
//! the event is made in decides whether it applies:
//!
//! - with no context on the task or thread at all, the span decides, as for code that never
//!   runs inside the framework's propagation;
//! - with a masked context, for work captured outside every invocation, no sink applies;
//! - with an entered context that runs in no scope instance, such as a job's or a lifecycle
//!   hook's, the span decides, since there is no chain to check it against;
//! - with an entered context, the span's sink applies when its instance is on the context's
//!   scope chain or is still opening under the context's innermost instance, as its
//!   constructors run before any context runs in it. Otherwise the innermost capturing
//!   instance on the context's chain applies, so work bound in an invocation and called inside
//!   a child instance that already opened logs to the invocation it was bound in.
//!
//! A sink whose instance finished no longer applies, and neither does one whose routing span
//! no layer took, such as under an explicit `upwell::scope=off`. An instance on the chain is
//! picked only when the caller `admits` its sink: the layer admits only sinks it took itself,
//! so a subscriber that never saw the routing span, or filtered it out, feeds no capture.

use std::sync::Arc;

use upwell_di::ScopeContainer;
use upwell_di::context::{ContextState, ScopeChain, with_context_state};

use super::ScopeLogSink;
use super::sink::any_open;

/// The sink an event made here belongs to, given `span_sink`, the sink of the innermost
/// capturing `upwell.scope` span around it, and which sinks of the context's chain `admits`.
pub(super) fn route(
    span_sink: Option<Arc<ScopeLogSink>>,
    admits: impl Fn(&ScopeLogSink) -> bool,
) -> Option<Arc<ScopeLogSink>> {
    if !any_open() {
        return None;
    }

    let span_sink = span_sink.filter(|sink| sink.is_open());
    let parent = span_sink.as_ref().and_then(|sink| sink.opening_under());
    let routed = with_context_state(|state| match state {
        ContextState::Absent => span_sink,
        ContextState::Masked => None,
        ContextState::Entered(chain) if chain.innermost().is_none() => span_sink,
        ContextState::Entered(chain) => entered(span_sink, parent.as_ref(), chain, admits),
    });

    drop(parent);

    routed
}

/// Hands `sink` to `container`, the capturing instance it belongs to, so work running in an
/// invocation context inside the instance finds it. The instance releases it with its last
/// handle. Only a scope instance a builder opened retains a sink, never a root, whose slot
/// holds its generation's guard. The instance opened, so from here on only the contexts that
/// run in it route to the sink.
pub(crate) fn retain_sink(container: &ScopeContainer, sink: Option<&Arc<ScopeLogSink>>) {
    let Some(sink) = sink else {
        return;
    };

    let retained = container.retain_drop_guard(Arc::clone(sink));

    sink.opened();

    // Ignored in release builds: a builder retains a sink once, right after it opened the
    // instance, so a second one cannot occur; the instance would keep the first.
    debug_assert!(retained.is_ok(), "a scope instance retains one log sink");
}

/// The sink an event belongs to inside an entered context whose scope chain is `chain`.
fn entered(
    span_sink: Option<Arc<ScopeLogSink>>,
    parent: Option<&Arc<ScopeContainer>>,
    chain: ScopeChain<'_>,
    admits: impl Fn(&ScopeLogSink) -> bool,
) -> Option<Arc<ScopeLogSink>> {
    match span_sink {
        Some(sink) if belongs(&sink, parent, chain) => Some(sink),
        Some(_) | None => chain
            .into_iter()
            .filter_map(retained)
            .find(|sink| admits(sink))
            .map(Arc::clone),
    }
}

/// Whether `sink`'s instance is on `chain`, or is still opening under `parent`, a descendant
/// of the chain's innermost instance.
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

/// The sink `instance` retains, while it captures: open, and attached to a routing span the
/// layer took, so a filter that disables the span disables capture on this path too.
fn retained(instance: &Arc<ScopeContainer>) -> Option<&Arc<ScopeLogSink>> {
    instance
        .drop_guard::<Arc<ScopeLogSink>>()
        .filter(|sink| sink.is_open() && sink.is_attached())
}
