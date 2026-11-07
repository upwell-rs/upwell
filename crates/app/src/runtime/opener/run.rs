//! Running planned work in a fresh scope instance: [`ScopeBuilder::run`].

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use pin_project_lite::pin_project;
use upwell_core::{DependencyDescriptor, Scope, ScopeId};
use upwell_di::{EntryPlan, FromContainer, ScopeContainer};

use super::builder::open_planned_in;
use super::{OpensUnder, ScopeBuilder};
use crate::entry::EntryTargets;
use crate::logs::{self, LogOptions, ScopeLogSink, ScopeLogStream};
use crate::runtime::{RuntimeView, scope_run_target};

/// Work [`ScopeBuilder::run`] runs inside a scope instance: a closure whose parameters are the
/// targets it injects, each a [`FromContainer`] type as an `#[init]` constructor takes, and
/// whose future is the work.
///
/// Implemented for closures of up to twelve parameters returning a future. The parameters
/// declare the work's dependencies the way an entry's do, so the work is planned like one.
pub trait ScopeWork<Targets>: Send + sealed::Sealed<Targets> {
    /// What the work returns.
    type Output;

    /// The type that keys the work's plan: two kinds of work with the same targets plan
    /// alike.
    fn plan_key() -> TypeId;

    /// The dependency edge of every target, in parameter order.
    fn targets() -> Vec<DependencyDescriptor>;

    /// Resolves the targets from `scope`, then runs the work.
    fn call(
        self,
        scope: &Arc<ScopeContainer>,
    ) -> impl Future<Output = crate::Result<Self::Output>> + Send;
}

/// Keeps [`ScopeWork`] implemented only for the closures this module covers, so how work is
/// planned and called can change without breaking downstream implementations.
mod sealed {
    /// Implemented only alongside each `ScopeWork` impl in this module.
    pub trait Sealed<Targets> {}
}

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
    /// the work there, returns its output and closes the instance once the work finishes
    /// (anything the work still shares keeps it alive).
    ///
    /// The work, its targets' resolution included, runs in an invocation context of its own
    /// whose innermost scope is the instance, and inside the instance's `upwell.scope` span.
    /// Work it carries into other tasks, threads or pools with Upwell's propagation primitives
    /// keeps logging to the instance, and sees it through [`ScopeLogs`](crate::logs::ScopeLogs).
    ///
    /// `work` is planned like an entry whose targets are its parameters: the instance builds
    /// what they need, and under [`deny_unplanned`](crate::AppBuilder::deny_unplanned) anything
    /// else built on demand fails. The plan is made the first time work with the same
    /// parameter types runs on a runtime generation and reused after that.
    ///
    /// ```ignore
    /// let synced = sync
    ///     .builder(CustomerSync { customer })
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
    ) -> ScopeRun<impl Future<Output = crate::Result<W::Output>> + use<'a, S, P, W, Targets>>
    where
        W: ScopeWork<Targets>,
    {
        let logs = Arc::new(RunLogs::new(S::ID, self.logs));

        ScopeRun {
            execution: execute(self, work, Arc::clone(&logs)),
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

async fn execute<S: OpensUnder<P>, P: Scope, W: ScopeWork<Targets>, Targets>(
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

    let (seeds, instance_logs) = builder.into_parts(&parent, sink);
    let instance = async {
        let container = open_planned_in(&view, S::ID, parent, Some(&plan), seeds).await?;

        logs::retain_sink(&container, instance_logs.sink());

        context
            .with_scope(Arc::clone(&container))
            .enter(work.call(&container))
            .await
    };
    let outcome = instance.instrument(instance_logs.span().clone()).await;

    instance_logs.finish();

    outcome
}

/// The generation, parent container and plan `builder`'s run opens with.
fn prepare<S: OpensUnder<P>, P: Scope, W: ScopeWork<Targets>, Targets>(
    builder: &ScopeBuilder<'_, S, P>,
) -> crate::Result<(RuntimeView, Arc<ScopeContainer>, Arc<EntryPlan>)> {
    let (view, parent) = builder.target()?;
    let plan = run_plan::<S, W, Targets>(&view)?;

    Ok((view, parent, plan))
}

/// The plan `view` keeps for work `W` into scope `S`, made on the first run.
fn run_plan<S: Scope, W: ScopeWork<Targets>, Targets>(
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

macro_rules! scope_work {
    ($($target:ident),*) => {
        impl<Work, Output, $($target),*> sealed::Sealed<($($target,)*)> for Work
        where
            Work: FnOnce($($target),*) -> Output + Send,
        {
        }

        impl<Work, Output, $($target),*> ScopeWork<($($target,)*)> for Work
        where
            Work: FnOnce($($target),*) -> Output + Send,
            Output: Future + Send,
            $($target: FromContainer + Send + 'static,)*
        {
            type Output = Output::Output;

            fn plan_key() -> TypeId {
                TypeId::of::<($($target,)*)>()
            }

            fn targets() -> Vec<DependencyDescriptor> {
                <($($target,)*) as EntryTargets>::targets()
            }

            #[allow(non_snake_case, unused_variables)]
            async fn call(self, scope: &Arc<ScopeContainer>) -> crate::Result<Self::Output> {
                $(let $target = scope.extract::<$target>().await?;)*

                Ok(self($($target),*).await)
            }
        }
    };
}

scope_work!();
scope_work!(A);
scope_work!(A, B);
scope_work!(A, B, C);
scope_work!(A, B, C, D);
scope_work!(A, B, C, D, E);
scope_work!(A, B, C, D, E, F);
scope_work!(A, B, C, D, E, F, G);
scope_work!(A, B, C, D, E, F, G, H);
scope_work!(A, B, C, D, E, F, G, H, I);
scope_work!(A, B, C, D, E, F, G, H, I, J);
scope_work!(A, B, C, D, E, F, G, H, I, J, K);
scope_work!(A, B, C, D, E, F, G, H, I, J, K, L);
