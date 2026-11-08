//! The spans entered on this thread while a sink is open, each with the sink of its innermost
//! capturing `upwell.scope` span, as the layer sees them enter and exit. A span with no
//! capturing ancestor, such as a root entered inside a capture, is recorded without a sink, so
//! it hides the capture around it just as it keeps its events out of it.
//!
//! [`ScopeLogs`](super::ScopeLogs) reads the innermost one here instead of looking the current
//! span up in the subscriber: that lookup would lock span extensions, which a formatter may
//! hold while it renders a value that asks for the log, and it needs the subscriber's concrete
//! registry type. This mirrors `tracing`'s own span stack and decides nothing: the invocation
//! context still decides whether the sink applies.

use std::cell::RefCell;
use std::sync::Arc;

use tracing::Id;

use super::ScopeLogSink;

thread_local! {
    static ENTERED: RefCell<Vec<(Id, Option<Arc<ScopeLogSink>>)>> = const { RefCell::new(Vec::new()) };
}

/// Records that the span `id`, routing to `sink`, or to no sink, was entered on this thread.
pub(super) fn enter(id: &Id, sink: Option<Arc<ScopeLogSink>>) {
    let entry = (id.clone(), sink);

    // Ignored when the thread is tearing down its locals: nothing reads the stack after that.
    let _ = ENTERED.try_with(|entered| entered.borrow_mut().push(entry));
}

/// Records that the span `id` exited on this thread, returning its sink so it drops outside
/// the stack.
pub(super) fn exit(id: &Id) -> Option<Arc<ScopeLogSink>> {
    ENTERED
        .try_with(|entered| {
            let mut entered = entered.borrow_mut();
            let position = entered.iter().rposition(|(entry, _)| entry == id)?;

            entered.remove(position).1
        })
        .ok()
        .flatten()
}

/// The sink the innermost span entered on this thread routes to.
pub(super) fn innermost() -> Option<Arc<ScopeLogSink>> {
    ENTERED
        .try_with(|entered| entered.borrow().last().and_then(|(_, sink)| sink.clone()))
        .ok()
        .flatten()
}
