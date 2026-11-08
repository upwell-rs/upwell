//! The spans entered on this thread while a sink is open, each with the layer that saw it enter
//! and the sink of its innermost capturing `upwell.scope` span, as the layers see them enter and
//! exit. A span with no capturing ancestor, such as a root entered inside a capture, is recorded
//! without a sink, so it hides the capture around it just as it keeps its events out of it.
//!
//! [`ScopeLogs`](super::ScopeLogs) reads the innermost one here instead of looking the current
//! span up in the subscriber: that lookup would lock span extensions, which a formatter may
//! hold while it renders a value that asks for the log, and it needs the subscriber's concrete
//! registry type. This mirrors `tracing`'s own span stack and decides nothing: the invocation
//! context still decides whether the sink applies.
//!
//! Span ids are unique only within one subscriber, so every entry carries the identity of the
//! layer that recorded it: one subscriber's exit never removes another's entry, and the log
//! reads only the entries of the subscriber its events reach.

use std::cell::RefCell;
use std::sync::Arc;

use tracing::Id;

use super::ScopeLogSink;
use super::reach::Reach;
use super::sink::LayerId;

thread_local! {
    static ENTERED: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
}

/// One span entered on this thread, as the layer `layer` saw it.
struct Entry {
    layer: LayerId,
    id: Id,
    sink: Option<Arc<ScopeLogSink>>,
}

/// Records that the layer `layer` saw the span `id`, routing to `sink`, or to no sink, entered
/// on this thread.
pub(super) fn enter(layer: LayerId, id: &Id, sink: Option<Arc<ScopeLogSink>>) {
    let entry = Entry {
        layer,
        id: id.clone(),
        sink,
    };

    // Ignored when the thread is tearing down its locals: nothing reads the stack after that.
    let _ = ENTERED.try_with(|entered| entered.borrow_mut().push(entry));
}

/// Records that the layer `layer` saw the span `id` exit on this thread, returning its sink so
/// it drops outside the stack.
pub(super) fn exit(layer: LayerId, id: &Id) -> Option<Arc<ScopeLogSink>> {
    ENTERED
        .try_with(|entered| {
            let mut entered = entered.borrow_mut();
            let position = entered
                .iter()
                .rposition(|entry| entry.layer == layer && entry.id == *id)?;

            entered.remove(position).sink
        })
        .ok()
        .flatten()
}

/// The sink the innermost span entered on this thread routes to, among the spans `reach`
/// covers: those the current subscriber's layer recorded. Inside a dispatch, which hides the
/// subscriber, it is the innermost span any layer recorded: the dispatching subscriber's,
/// unless spans of several subscribers interleave on this thread.
pub(super) fn innermost(reach: Reach) -> Option<Arc<ScopeLogSink>> {
    ENTERED
        .try_with(|entered| {
            let entered = entered.borrow();
            let innermost = match reach {
                Reach::Layer(layer) => entered.iter().rfind(|entry| entry.layer == layer),
                Reach::Any => entered.last(),
                Reach::Nothing => None,
            };

            innermost.and_then(|entry| entry.sink.clone())
        })
        .ok()
        .flatten()
}
