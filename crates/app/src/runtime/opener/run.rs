//! Running planned work in a fresh scope instance: [`ScopeBuilder::run`].

use std::any::TypeId;
use std::future::Future;
use std::sync::Arc;

use upwell_core::{DependencyDescriptor, Scope};
use upwell_di::{EntryPlan, FromContainer, ScopeContainer};

use super::builder::open_planned_in;
use super::{OpensUnder, ScopeBuilder};
use crate::entry::EntryTargets;
use crate::runtime::RuntimeView;

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

impl<S: OpensUnder<P>, P: Scope> ScopeBuilder<'_, S, P> {
    /// Opens the instance for `work`, runs it there and returns its output, closing the
    /// instance once the work finishes (anything the work still shares keeps it alive).
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
    /// Fails as [`open`](Self::open) does, and when a target fails to resolve.
    pub async fn run<W, Targets>(self, work: W) -> crate::Result<W::Output>
    where
        W: ScopeWork<Targets>,
    {
        let (view, parent) = self.target()?;
        let plan = run_plan::<S, W, Targets>(&view)?;
        let container = open_planned_in(&view, S::ID, parent, Some(&plan), self.seeds).await?;

        work.call(&container).await
    }
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
