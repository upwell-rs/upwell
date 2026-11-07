//! The capturing `upwell.scope` spans entered on this thread, as the layer sees them enter and
//! exit.
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
    static ENTERED: RefCell<Vec<(Id, Arc<ScopeLogSink>)>> = const { RefCell::new(Vec::new()) };
}

/// Records that the span `id`, routing to `sink`, was entered on this thread.
pub(super) fn enter(id: &Id, sink: Arc<ScopeLogSink>) {
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

            Some(entered.remove(position).1)
        })
        .ok()
        .flatten()
}

/// The sink of the innermost capturing span entered on this thread.
pub(super) fn innermost() -> Option<Arc<ScopeLogSink>> {
    ENTERED
        .try_with(|entered| entered.borrow().last().map(|(_, sink)| Arc::clone(sink)))
        .ok()
        .flatten()
}
