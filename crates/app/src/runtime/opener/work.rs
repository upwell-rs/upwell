//! The work a scope instance runs: [`ScopeWork`] for [`ScopeBuilder::run`] and [`TryScopeWork`]
//! for [`ScopeBuilder::try_run`].

use std::any::TypeId;
use std::future::Future;
use std::sync::Arc;

use upwell_core::DependencyDescriptor;
use upwell_di::{FromContainer, ScopeContainer};

use super::ScopeEndKind;
use crate::entry::EntryTargets;

/// Work [`ScopeBuilder::run`] runs inside a scope instance: a closure whose parameters are the
/// targets it injects, each a [`FromContainer`] type as an `#[init]` constructor takes, and
/// whose future is the work and resolves to `()`.
///
/// Implemented for closures of up to twelve parameters returning a future of `()`. The
/// parameters declare the work's dependencies the way an entry's do, so the work is planned
/// like one. Work that returns a value or a `Result` runs through
/// [`ScopeBuilder::try_run`] and [`TryScopeWork`].
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not work `ScopeBuilder::run` can run",
    label = "`run` takes a closure of injectable targets returning a future of `()`",
    note = "work that returns a value or a `Result` runs through `ScopeBuilder::try_run`"
)]
pub trait ScopeWork<Targets>: Send + sealed::Sealed<Targets> {
    /// The type that keys the work's plan: two kinds of work with the same targets plan
    /// alike.
    fn plan_key() -> TypeId;

    /// The dependency edge of every target, in parameter order.
    fn targets() -> Vec<DependencyDescriptor>;

    /// Resolves the targets from `scope`, then runs the work.
    fn call(self, scope: &Arc<ScopeContainer>) -> impl Future<Output = crate::Result<()>> + Send;
}

/// Work [`ScopeBuilder::try_run`] runs inside a scope instance: as [`ScopeWork`], but its
/// future resolves to a `Result`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not work `ScopeBuilder::try_run` can run",
    label = "`try_run` takes a closure of injectable targets returning a future of a `Result`",
    note = "work that returns `()` runs through `ScopeBuilder::run`"
)]
pub trait TryScopeWork<Targets>: Send + sealed::Sealed<Targets> {
    /// What the work returns on success.
    type Output;

    /// What the work returns on failure.
    type Error;

    /// The type that keys the work's plan: two kinds of work with the same targets plan
    /// alike.
    fn plan_key() -> TypeId;

    /// The dependency edge of every target, in parameter order.
    fn targets() -> Vec<DependencyDescriptor>;

    /// Resolves the targets from `scope`, then runs the work.
    fn call(
        self,
        scope: &Arc<ScopeContainer>,
    ) -> impl Future<Output = crate::Result<Result<Self::Output, Self::Error>>> + Send;
}

/// Keeps the work traits implemented only for the closures this module covers, so how work is
/// planned and called can change without breaking downstream implementations.
mod sealed {
    /// Implemented only alongside each work impl in this module.
    pub trait Sealed<Targets> {}
}

/// What [`execute`] needs of either kind of work, so one execution serves `run` and `try_run`.
pub(super) trait Planned<Targets>: Send {
    type Output;

    fn plan_key() -> TypeId;

    fn targets() -> Vec<DependencyDescriptor>;

    fn call(
        self,
        scope: &Arc<ScopeContainer>,
    ) -> impl Future<Output = crate::Result<Self::Output>> + Send;

    fn ending(output: &Self::Output) -> ScopeEndKind;
}

/// Work that returns `()`.
pub(super) struct Unit<W>(pub(super) W);

/// Work that returns a `Result`.
pub(super) struct Fallible<W>(pub(super) W);

impl<W: ScopeWork<Targets>, Targets> Planned<Targets> for Unit<W> {
    type Output = ();

    fn plan_key() -> TypeId {
        W::plan_key()
    }

    fn targets() -> Vec<DependencyDescriptor> {
        W::targets()
    }

    fn call(
        self,
        scope: &Arc<ScopeContainer>,
    ) -> impl Future<Output = crate::Result<Self::Output>> + Send {
        self.0.call(scope)
    }

    fn ending(_output: &Self::Output) -> ScopeEndKind {
        ScopeEndKind::Completed
    }
}

impl<W: TryScopeWork<Targets>, Targets> Planned<Targets> for Fallible<W> {
    type Output = Result<W::Output, W::Error>;

    fn plan_key() -> TypeId {
        W::plan_key()
    }

    fn targets() -> Vec<DependencyDescriptor> {
        W::targets()
    }

    fn call(
        self,
        scope: &Arc<ScopeContainer>,
    ) -> impl Future<Output = crate::Result<Self::Output>> + Send {
        self.0.call(scope)
    }

    fn ending(output: &Self::Output) -> ScopeEndKind {
        match output {
            Ok(_) => ScopeEndKind::Completed,
            Err(_) => ScopeEndKind::Errored {
                error_type: std::any::type_name::<W::Error>(),
            },
        }
    }
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
            Output: Future<Output = ()> + Send,
            $($target: FromContainer + Send + 'static,)*
        {
            fn plan_key() -> TypeId {
                TypeId::of::<($($target,)*)>()
            }

            fn targets() -> Vec<DependencyDescriptor> {
                <($($target,)*) as EntryTargets>::targets()
            }

            #[allow(non_snake_case, unused_variables)]
            async fn call(self, scope: &Arc<ScopeContainer>) -> crate::Result<()> {
                $(let $target = scope.extract::<$target>().await?;)*

                self($($target),*).await;

                Ok(())
            }
        }

        impl<Work, Output, Success, Failure, $($target),*> TryScopeWork<($($target,)*)> for Work
        where
            Work: FnOnce($($target),*) -> Output + Send,
            Output: Future<Output = Result<Success, Failure>> + Send,
            $($target: FromContainer + Send + 'static,)*
        {
            type Output = Success;
            type Error = Failure;

            fn plan_key() -> TypeId {
                TypeId::of::<($($target,)*)>()
            }

            fn targets() -> Vec<DependencyDescriptor> {
                <($($target,)*) as EntryTargets>::targets()
            }

            #[allow(non_snake_case, unused_variables)]
            async fn call(
                self,
                scope: &Arc<ScopeContainer>,
            ) -> crate::Result<Result<Success, Failure>> {
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
