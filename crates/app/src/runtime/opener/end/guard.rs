//! The owner of an instance's log that fires [`ScopeEnded`] when it is dropped.

use std::marker::PhantomData;
use std::mem;
use std::sync::Arc;

use tokio::runtime::Handle;
use tracing::instrument::WithSubscriber;
use tracing::{Dispatch, Instrument, Span};
use upwell_core::{Scope, ScopeId, TypeDescriptor};
use upwell_di::ScopeContainer;
use upwell_hooks::HookKind;

use super::{AnyScopeEnded, ScopeEnd, ScopeEndContext, ScopeEndKind, ScopeEnded};
use crate::logs::{self, InstanceLogs, LogField, SCOPE_LOG_TARGET, ScopeLogSink};
use crate::runtime::generation::EndTask;
use crate::runtime::transition_hooks::{HookOwners, failed_observers};
use crate::runtime::{RuntimeView, hook_target};

/// The log of one instance and, while an end hook listens, what firing it takes.
///
/// Dropping it ends the instance, if it was armed: the log closes at once when nothing
/// listens, or after the hooks ran on a spawned task that owns the log until then. An end that
/// was never armed, because its instance never opened, fires nothing.
pub(in crate::runtime) struct InstanceEnd<S: Scope> {
    logs: InstanceLogs,
    firing: Option<Firing<S>>,
    unfinished: Option<ScopeEndKind>,
    finished: Option<ScopeEndKind>,
}

/// What the hooks of one ended instance need, held only when a hook listens.
struct Firing<S: Scope> {
    view: RuntimeView,
    scope: ScopeId,
    name: &'static str,
    params: Arc<[LogField]>,
    kinds: PhantomData<fn() -> S>,
}

impl<S: Scope> InstanceEnd<S> {
    /// The end of an instance of `S` with the value `scope` on `view`. It ends as `unfinished`
    /// unless [`complete`](Self::complete) says otherwise, and fires nothing while that is
    /// `None` until [`arm`](Self::arm) sets it.
    pub(in crate::runtime) fn new(
        view: &RuntimeView,
        logs: InstanceLogs,
        scope: &S,
        unfinished: Option<ScopeEndKind>,
    ) -> Self {
        let hooks = view.hooks();
        let observed = hooks.has::<ScopeEnded<S>>() || hooks.has::<AnyScopeEnded>();
        let firing = observed.then(|| Firing {
            view: view.clone(),
            scope: S::ID,
            name: S::NAME,
            params: logs::loggable_params(scope),
            kinds: PhantomData,
        });

        Self {
            logs,
            firing,
            unfinished,
            finished: None,
        }
    }

    /// The instance's span, to instrument its work with.
    pub(in crate::runtime) fn span(&self) -> &Span {
        self.logs.span()
    }

    /// The instance's sink, when it captures.
    pub(in crate::runtime) fn sink(&self) -> Option<&Arc<ScopeLogSink>> {
        self.logs.sink()
    }

    /// Whether dropping this ends something anyone observes: a log or a hook.
    pub(in crate::runtime) fn is_observed(&self) -> bool {
        self.logs.sink().is_some() || self.firing.is_some()
    }

    /// Makes the end fire as `kind` if it is dropped without being completed, once the
    /// instance exists.
    pub(in crate::runtime) fn arm(&mut self, kind: ScopeEndKind) {
        self.unfinished = Some(kind);
    }

    /// Hands the end to `container`, which releases it with its last owner, and returns the
    /// span and sink the caller's handles keep. An end nothing observes is dropped.
    pub(in crate::runtime) fn retain_in(self, container: &ScopeContainer) -> Observed {
        let observed = Observed {
            sink: self.sink().cloned(),
            span: self.span().clone(),
        };

        if self.is_observed() {
            logs::retain(container, observed.sink.clone(), Some(Box::new(self)));
        }

        observed
    }

    /// Ends the instance as `kind`.
    pub(in crate::runtime) fn complete(mut self, kind: ScopeEndKind) {
        self.finished = Some(kind);
    }
}

/// What an instance's handles keep of its end: the span to instrument its work with and the
/// sink to subscribe to.
pub(in crate::runtime) struct Observed {
    pub(in crate::runtime) sink: Option<Arc<ScopeLogSink>>,
    pub(in crate::runtime) span: Span,
}

impl<S: Scope> Drop for InstanceEnd<S> {
    fn drop(&mut self) {
        let Some(kind) = self.finished.or(self.unfinished) else {
            return;
        };
        let Some(firing) = self.firing.take() else {
            return;
        };
        let logs = mem::replace(&mut self.logs, InstanceLogs::none());

        firing.spawn(kind, logs);
    }
}

impl<S: Scope> Firing<S> {
    /// Runs the hooks on the ambient Tokio runtime, or traces that they were skipped.
    fn spawn(self, kind: ScopeEndKind, logs: InstanceLogs) {
        let Ok(runtime) = Handle::try_current() else {
            tracing::warn!(
                target: SCOPE_LOG_TARGET,
                scope = %self.scope,
                kind = kind.as_str(),
                "scope end hooks skipped: no async runtime"
            );

            return;
        };

        let task = self.view.track_end_task();
        let dispatch = tracing::dispatcher::get_default(Dispatch::clone);

        runtime.spawn(self.fire(kind, logs, task).with_subscriber(dispatch));
    }

    /// Fires [`ScopeEnded`] inside the instance's span, then lets the log close.
    async fn fire(self, kind: ScopeEndKind, logs: InstanceLogs, task: EndTask) {
        let span = logs.span().clone();

        self.fire_hooks(kind).instrument(span).await;

        logs.finish();
        drop(task);
    }

    async fn fire_hooks(&self, kind: ScopeEndKind) {
        let cx = ScopeEndContext::new(ScopeEnd {
            scope: self.scope,
            name: self.name,
            kind,
            params: Arc::clone(&self.params),
        });
        let owners = HookOwners::new(self.view.resolved_components());
        let hooks = self.view.hooks();
        let typed = self.view.context(hook_target!(ScopeEnded::<S>::NAME));
        let any = self.view.context(hook_target!(AnyScopeEnded::NAME));
        let typed = typed.enter(hooks.fire_all::<ScopeEnded<S>>(&cx)).await;
        let any = any.enter(hooks.fire_all::<AnyScopeEnded>(&cx)).await;

        self.trace_failures(kind, ScopeEnded::<S>::NAME, &owners, typed.observers);
        self.trace_failures(kind, AnyScopeEnded::NAME, &owners, any.observers);
    }

    fn trace_failures(
        &self,
        kind: ScopeEndKind,
        hook: &'static str,
        owners: &HookOwners,
        observers: Vec<(TypeDescriptor, upwell_hooks::Result<()>)>,
    ) {
        for (owner, outcome) in failed_observers(observers, owners) {
            tracing::warn!(
                target: SCOPE_LOG_TARGET,
                scope = %self.scope,
                kind = kind.as_str(),
                hook,
                owner,
                outcome = outcome.as_str(),
                "scope end hook failed"
            );
        }
    }
}
