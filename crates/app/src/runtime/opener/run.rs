//! Running planned work in a fresh scope instance: [`ScopeBuilder::run`].

use std::any::TypeId;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll, ready};

use futures::future::BoxFuture;
use upwell_core::{DependencyDescriptor, Scope};
use upwell_di::{EntryPlan, FromContainer, ScopeContainer};

use super::builder::open_planned_in;
use super::{OpensUnder, ScopeBuilder};
use crate::entry::EntryTargets;
use crate::logs::{self, ScopeLogSink, ScopeLogStream};
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

/// Work waiting to run in a fresh scope instance, made by [`ScopeBuilder::run`]. Awaiting it
/// opens the instance, runs the work there and returns its output.
///
/// Before that, [`logs`](Self::logs) subscribes to the instance's log, so a stream taken
/// ahead of the run sees every record.
#[must_use = "a scope run does nothing until it is awaited"]
pub struct ScopeRun<'a, S: Scope, P: Scope, W: ScopeWork<Targets>, Targets> {
    ready: Option<(ScopeBuilder<'a, S, P>, W)>,
    running: Option<BoxFuture<'a, crate::Result<W::Output>>>,
    sink: OnceLock<Arc<ScopeLogSink>>,
    targets: PhantomData<fn() -> Targets>,
}

impl<S: Scope, P: Scope, W: ScopeWork<Targets>, Targets> Unpin for ScopeRun<'_, S, P, W, Targets> {}

impl<'a, S: OpensUnder<P>, P: Scope> ScopeBuilder<'a, S, P> {
    /// Prepares `work` to run in a fresh instance: awaiting the result opens the instance, runs
    /// the work there, returns its output and closes the instance once the work finishes
    /// (anything the work still shares keeps it alive).
    ///
    /// The work, its targets' resolution included, runs in an invocation context of its own
    /// whose innermost scope is the instance, and inside the instance's `upwell.scope` span.
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
    pub fn run<W, Targets>(self, work: W) -> ScopeRun<'a, S, P, W, Targets>
    where
        W: ScopeWork<Targets>,
    {
        let sink = OnceLock::new();

        if self.logs.capture {
            sink.get_or_init(|| logs::new_sink(&self.params, &self.logs));
        }

        ScopeRun {
            ready: Some((self, work)),
            running: None,
            sink,
            targets: PhantomData,
        }
    }
}

impl<S: Scope, P: Scope, W: ScopeWork<Targets>, Targets> ScopeRun<'_, S, P, W, Targets> {
    /// A stream of the instance's log, from the first record when taken before the run is
    /// polled. It ends when the instance closes, so collecting it yields the whole log. Needs a
    /// [`ScopeLogLayer`](crate::logs::ScopeLogLayer) in the subscriber.
    pub fn logs(&self) -> ScopeLogStream {
        let Some((builder, _)) = &self.ready else {
            return self
                .sink
                .get()
                .map_or_else(ScopeLogStream::ended, |sink| sink.subscribe());
        };

        self.sink
            .get_or_init(|| logs::new_sink(&builder.params, &builder.logs))
            .subscribe()
    }
}

impl<'a, S, P, W, Targets> Future for ScopeRun<'a, S, P, W, Targets>
where
    S: OpensUnder<P>,
    P: Scope,
    W: ScopeWork<Targets> + 'a,
    Targets: 'a,
{
    type Output = crate::Result<W::Output>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        if let Some((builder, work)) = this.ready.take() {
            let sink = this.sink.get().map(Arc::clone);

            this.running = Some(Box::pin(execute(builder, work, sink)));
        }

        let running = this
            .running
            .as_mut()
            .expect("a scope run is polled after it completed");
        let outcome = ready!(running.as_mut().poll(cx));

        this.running = None;

        Poll::Ready(outcome)
    }
}

async fn execute<S: OpensUnder<P>, P: Scope, W: ScopeWork<Targets>, Targets>(
    builder: ScopeBuilder<'_, S, P>,
    work: W,
    sink: Option<Arc<ScopeLogSink>>,
) -> crate::Result<W::Output> {
    use tracing::Instrument;

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
    let (seeds, instance_logs) = builder.into_parts(sink);
    let instance = async {
        let container = open_planned_in(&view, S::ID, parent, Some(&plan), seeds).await?;

        context
            .with_scope(Arc::clone(&container))
            .enter(work.call(&container))
            .await
    };
    let outcome = logs::with_current_sink(
        instance_logs.sink().map(Arc::clone),
        instance.instrument(instance_logs.span().clone()),
    )
    .await;

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
