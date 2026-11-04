//! The `upwell.scope` span of one instance and how its sink reaches the layer.

use std::cell::RefCell;
use std::sync::Arc;

use tracing::{Level, Span};
use upwell_core::Scope;

use super::render::{Rendered, collect};
use super::{LogOptions, SCOPE_LOG_TARGET, ScopeLogSink};

thread_local! {
    static HANDOFF: RefCell<Option<Arc<ScopeLogSink>>> = const { RefCell::new(None) };
}

/// The span and sink of one scope instance.
///
/// Dropping it closes the sink, ending every stream, so an instance shared by several owners
/// keeps its logs open for as long as the `Arc` around this value lives.
pub struct InstanceLogs {
    span: Span,
    sink: Option<Arc<ScopeLogSink>>,
}

impl InstanceLogs {
    /// No span and no sink, for an instance nothing observes.
    pub fn none() -> Self {
        Self {
            span: Span::none(),
            sink: None,
        }
    }

    /// The span of an instance of `S` with the value `scope`, creating the sink when
    /// `options` ask for capture or `sink` was made earlier.
    pub(crate) fn open<S: Scope>(
        scope: &S,
        options: &LogOptions,
        sink: Option<Arc<ScopeLogSink>>,
    ) -> Self {
        let sink = sink.or_else(|| options.capture.then(|| new_sink(scope, options)));

        Self {
            span: scope_span(scope, sink.as_ref()),
            sink,
        }
    }

    /// The instance's span, to instrument its work with.
    pub fn span(&self) -> &Span {
        &self.span
    }

    /// The instance's sink, when it captures.
    pub fn sink(&self) -> Option<&Arc<ScopeLogSink>> {
        self.sink.as_ref()
    }

    /// Ends the instance's logs: every stream yields what is buffered and finishes. The
    /// instance's work, and an end-of-scope hook, run before it.
    pub fn finish(&self) {
        if let Some(sink) = &self.sink {
            sink.close();
        }
    }
}

impl Drop for InstanceLogs {
    fn drop(&mut self) {
        self.finish();
    }
}

/// A sink for an instance of `S` with the value `scope`.
pub(crate) fn new_sink<S: Scope>(scope: &S, options: &LogOptions) -> Arc<ScopeLogSink> {
    Arc::new(ScopeLogSink::new(S::ID, collect(scope).into(), options))
}

/// The `upwell.scope` span of an instance. It is at error level so a filter that hides
/// everything but failures still opens it: a hidden span could not route its events.
///
/// While the span is created, `sink` waits in a thread-local for the layer's `on_new_span`
/// to take it into the span's extensions.
fn scope_span<S: Scope>(scope: &S, sink: Option<&Arc<ScopeLogSink>>) -> Span {
    let create = || {
        tracing::span!(
            target: SCOPE_LOG_TARGET,
            Level::ERROR,
            "upwell.scope",
            scope.id = %S::ID,
            scope.name = S::NAME,
            params = %Rendered(scope),
        )
    };

    let Some(sink) = sink else {
        return create();
    };

    HANDOFF.with_borrow_mut(|slot| *slot = Some(Arc::clone(sink)));

    let span = create();

    HANDOFF.with_borrow_mut(Option::take);

    span
}

/// Takes the sink waiting for the span being created, if any.
#[cfg(feature = "tracing-subscriber")]
pub(super) fn take_handoff() -> Option<Arc<ScopeLogSink>> {
    HANDOFF.with_borrow_mut(Option::take)
}
