//! The owner of an instance's log that fires [`ScopeEnded`] when it is dropped.

use std::mem;
use std::sync::Arc;

use tokio::runtime::Handle;
use tracing::{Instrument, Span};
use upwell_core::{Scope, ScopeId};
use upwell_hooks::HookKind;

use super::{ScopeEnd, ScopeEndContext, ScopeEndKind, ScopeEnded};
use crate::logs::{self, InstanceLogs, LogField, SCOPE_LOG_TARGET, ScopeLogSink};
use crate::runtime::transition_hooks::{HookOwners, failed_observers};
use crate::runtime::{RuntimeView, hook_target};

/// The log of one instance and, while a [`ScopeEnded`] hook listens, what firing it takes.
///
/// Dropping it ends the instance: the log closes at once when nothing listens, or after the
/// hooks ran on a spawned task that owns the log until then.
pub(in crate::runtime) struct InstanceEnd {
    logs: InstanceLogs,
    firing: Option<Firing>,
    unfinished: ScopeEndKind,
    finished: Option<ScopeEndKind>,
}

/// What the hooks of one ended instance need, held only when a hook listens.
struct Firing {
    view: RuntimeView,
    scope: ScopeId,
    name: &'static str,
    params: Arc<[LogField]>,
}

impl InstanceEnd {
    /// The end of an instance of `S` with the value `scope` on `view`, ending as `unfinished`
    /// unless [`complete`](Self::complete) says otherwise.
    pub(in crate::runtime) fn new<S: Scope>(
        view: &RuntimeView,
        logs: InstanceLogs,
        scope: &S,
        unfinished: ScopeEndKind,
    ) -> Self {
        let firing = view.hooks().has::<ScopeEnded>().then(|| Firing {
            view: view.clone(),
            scope: S::ID,
            name: S::NAME,
            params: logs::loggable_params(scope),
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

    /// Ends the instance as `kind`.
    pub(in crate::runtime) fn complete(mut self, kind: ScopeEndKind) {
        self.finished = Some(kind);
    }
}

impl Drop for InstanceEnd {
    fn drop(&mut self) {
        let Some(firing) = self.firing.take() else {
            return;
        };
        let kind = self.finished.unwrap_or(self.unfinished);
        let logs = mem::replace(&mut self.logs, InstanceLogs::none());

        firing.spawn(kind, logs);
    }
}

impl Firing {
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

        runtime.spawn(self.fire(kind, logs));
    }

    /// Fires [`ScopeEnded`] inside the instance's span, then lets the log close.
    async fn fire(self, kind: ScopeEndKind, logs: InstanceLogs) {
        let span = logs.span().clone();

        self.fire_hooks(kind).instrument(span).await;

        logs.finish();
    }

    async fn fire_hooks(&self, kind: ScopeEndKind) {
        let cx = ScopeEndContext::new(ScopeEnd {
            scope: self.scope,
            name: self.name,
            kind,
            params: Arc::clone(&self.params),
        });
        let owners = HookOwners::new(self.view.resolved_components());
        let context = self.view.context(hook_target!(ScopeEnded::NAME));
        let fired = context
            .enter(self.view.hooks().fire_all::<ScopeEnded>(&cx))
            .await;

        for (owner, outcome) in failed_observers(fired.observers, &owners) {
            tracing::warn!(
                target: SCOPE_LOG_TARGET,
                scope = %self.scope,
                kind = kind.as_str(),
                hook = ScopeEnded::NAME,
                owner,
                outcome = outcome.as_str(),
                "scope end hook failed"
            );
        }
    }
}
