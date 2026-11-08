//! Running planned work in a fresh scope instance: [`ScopeBuilder::run`].

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use pin_project_lite::pin_project;
use upwell_core::{Scope, ScopeId};
use upwell_di::{EntryPlan, ScopeContainer};

use super::builder::open_planned_in;
use super::work::{Fallible, Planned, Unit};
use super::{OpensUnder, RunError, ScopeBuilder, ScopeEndKind, ScopeWork, TryScopeWork};
use crate::logs::{self, LogOptions, ScopeLogSink, ScopeLogStream};
use crate::runtime::{RuntimeView, scope_run_target};

pin_project! {
    /// Work waiting to run in a fresh scope instance, made by [`ScopeBuilder::run`]. Awaiting it
    /// opens the instance, runs the work there and returns its output.
    ///
    /// Before that, [`logs`](Self::logs) subscribes to the instance's log, so a stream taken
    /// ahead of the run sees every record.
    #[must_use = "a scope run does nothing until it is awaited"]
    pub struct ScopeRun<F> {
        #[pin]
        execution: F,
        logs: Arc<RunLogs>,
    }
}

/// The sink of one run, shared by its handle and its execution, so a stream taken from either
/// side listens to what the instance captures.
struct RunLogs {
    scope: ScopeId,
    options: LogOptions,
    state: Mutex<RunLogState>,
}

/// Whether the execution started, and the sink it captures into.
struct RunLogState {
    started: bool,
    sink: Option<Arc<ScopeLogSink>>,
}

impl RunLogs {
    fn new(scope: ScopeId, options: LogOptions) -> Self {
        Self {
            scope,
            options,
            state: Mutex::new(RunLogState {
                started: false,
                sink: None,
            }),
        }
    }

    /// The sink to subscribe to: the run's own once it captures, a new one while the run has not
    /// started, none after it started without capturing.
    fn subscribable(&self) -> Option<Arc<ScopeLogSink>> {
        let mut state = self.state.lock().expect("run log state lock not poisoned");

        if state.sink.is_none() && !state.started {
            state.sink = Some(logs::deferred_sink(self.scope, &self.options));
        }

        state.sink.clone()
    }

    /// Marks the execution started and returns the sink it captures into, made when the options
    /// ask for capture and no stream was taken before.
    fn start(&self) -> Option<Arc<ScopeLogSink>> {
        let mut state = self.state.lock().expect("run log state lock not poisoned");

        state.started = true;

        if state.sink.is_none() && self.options.capture {
            state.sink = Some(logs::deferred_sink(self.scope, &self.options));
        }

        state.sink.clone()
    }
}

impl<'a, S: OpensUnder<P>, P: Scope> ScopeBuilder<'a, S, P> {
    /// Prepares `work` to run in a fresh instance: awaiting the result opens the instance, runs
    /// the work there and closes the instance once the work finishes (anything the work still
    /// shares keeps it alive).
    ///
    /// The instance opens, and the work runs, its targets' resolution included, in an invocation
    /// context of its own: under the instance's parent while it opens, then with exactly this
    /// instance as its innermost scope, even when its construction opens another instance of
    /// the same scope, and inside the instance's `upwell.scope` span.
    /// Work it carries into other tasks, threads or pools with Upwell's propagation primitives
    /// keeps logging to the instance, and sees it through [`ScopeLogs`](crate::logs::ScopeLogs).
    ///
    /// `work` is planned like an entry whose targets are its parameters: the instance builds
    /// what they need, and under [`deny_unplanned`](crate::AppBuilder::deny_unplanned) anything
    /// else built on demand fails. The plan is made the first time work with the same
    /// parameter types runs on a runtime generation and reused after that.
    ///
    /// The work returns `()`; work that returns a value or a `Result` runs through
    /// [`try_run`](Self::try_run). The instance ends as
    /// [`Completed`](ScopeEndKind::Completed), or [`Failed`](ScopeEndKind::Failed) when it
    /// could not open or a target could not resolve, or [`Cancelled`](ScopeEndKind::Cancelled)
    /// when the run is dropped first.
    ///
    /// ```ignore
    /// sync.builder(CustomerSync { customer })
    ///     .run(|client: Arc<CustomerClient>| async move { client.sync().await })
    ///     .await?;
    /// ```
    ///
    /// # Errors
    ///
    /// Awaiting it fails as [`open`](Self::open) does, and when a target fails to resolve.
    pub fn run<W, Targets>(
        self,
        work: W,
    ) -> ScopeRun<impl Future<Output = crate::Result<()>> + use<'a, S, P, W, Targets>>
    where
        W: ScopeWork<Targets>,
    {
        let logs = Arc::new(RunLogs::new(S::ID, self.logs));

        ScopeRun {
            execution: execute(self, Unit(work), Arc::clone(&logs)),
            logs,
        }
    }

    /// As [`run`](Self::run), for work that returns a `Result`: its success is returned, its
    /// failure comes back as [`RunError::Work`], and a failure to open the instance or resolve
    /// a target as [`RunError::Scope`].
    ///
    /// The instance ends as [`Completed`](ScopeEndKind::Completed) when the work returned `Ok`,
    /// and as [`Errored`](ScopeEndKind::Errored) when it returned `Err`, which records only the
    /// error's type, never its value.
    ///
    /// ```ignore
    /// let synced = sync
    ///     .builder(CustomerSync { customer })
    ///     .try_run(|client: Arc<CustomerClient>| async move { client.sync().await })
    ///     .await?;
    /// ```
    ///
    /// # Errors
    ///
    /// [`RunError::Scope`] as [`run`](Self::run) fails, and [`RunError::Work`] with the work's
    /// own error.
    pub fn try_run<W, Targets>(
        self,
        work: W,
    ) -> ScopeRun<
        impl Future<Output = Result<W::Output, RunError<W::Error>>> + use<'a, S, P, W, Targets>,
    >
    where
        W: TryScopeWork<Targets>,
    {
        let logs = Arc::new(RunLogs::new(S::ID, self.logs));
        let execution = execute(self, Fallible(work), Arc::clone(&logs));

        ScopeRun {
            execution: async move {
                match execution.await {
                    Ok(Ok(output)) => Ok(output),
                    Ok(Err(error)) => Err(RunError::Work(error)),
                    Err(error) => Err(RunError::Scope(error)),
                }
            },
            logs,
        }
    }
}

impl<F> ScopeRun<F> {
    /// A stream of the instance's log, from the first record when taken before the run is
    /// polled. It ends when the instance closes, so collecting it yields the whole log. Needs a
    /// [`ScopeLogLayer`](crate::logs::ScopeLogLayer) in the subscriber.
    pub fn logs(&self) -> ScopeLogStream {
        self.logs
            .subscribable()
            .map_or_else(ScopeLogStream::ended, |sink| sink.subscribe())
    }
}

impl<F: Future> Future for ScopeRun<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().execution.poll(cx)
    }
}

async fn execute<S: OpensUnder<P>, P: Scope, W: Planned<Targets>, Targets>(
    builder: ScopeBuilder<'_, S, P>,
    work: W,
    run_logs: Arc<RunLogs>,
) -> crate::Result<W::Output> {
    use tracing::Instrument;

    let sink = run_logs.start();
    let (view, parent, plan) = match prepare::<S, P, W, Targets>(&builder) {
        Ok(prepared) => prepared,
        Err(error) => {
            if let Some(sink) = &sink {
                sink.close();
            }

            return Err(error);
        }
    };
    let context = builder.opener.context(scope_run_target!(S::ID));

    if let Some(sink) = &sink {
        logs::describe_sink(sink, &builder.params);
    }

    let (seeds, instance_end) =
        builder.into_parts(&view, Some(ScopeEndKind::Cancelled), &parent, sink);
    let opening = context
        .clone()
        .with_opening_scope(S::KEY, Arc::clone(&parent));
    let instance = async {
        let container = open_planned_in(&view, S::ID, parent, Some(&plan), seeds).await?;

        logs::retain(&container, instance_end.sink().cloned(), None);

        context
            .with_scope(Arc::clone(&container))
            .enter(work.call(&container))
            .await
    };
    let outcome = opening
        .enter(instance)
        .instrument(instance_end.span().clone())
        .await;

    instance_end.complete(match &outcome {
        Ok(output) => W::ending(output),
        Err(_) => ScopeEndKind::Failed,
    });

    outcome
}

/// The generation, parent container and plan `builder`'s run opens with.
fn prepare<S: OpensUnder<P>, P: Scope, W: Planned<Targets>, Targets>(
    builder: &ScopeBuilder<'_, S, P>,
) -> crate::Result<(RuntimeView, Arc<ScopeContainer>, Arc<EntryPlan>)> {
    let (view, parent) = builder.target()?;
    let plan = run_plan::<S, W, Targets>(&view)?;

    Ok((view, parent, plan))
}

/// The plan `view` keeps for work `W` into scope `S`, made on the first run.
fn run_plan<S: Scope, W: Planned<Targets>, Targets>(
    view: &RuntimeView,
) -> crate::Result<Arc<EntryPlan>> {
    let scope_plan = view.scope_plan();

    scope_plan
        .layouts
        .run_plan(
            S::ID,
            W::plan_key(),
            W::targets,
            view.effective_graph().provider_selection(),
            scope_plan.topology(),
        )
        .ok_or(crate::Error::UndeclaredScopeOpen { scope: S::ID })
}
