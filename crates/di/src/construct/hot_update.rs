//! Explicit in-place hot updates.
//!
//! A component opts in by declaring a hot-update preparation: an async function that
//! reads the active instance through [`Previous`] plus candidate dependencies and returns
//! an owned [`HotUpdate`] commit action. Preparation is fallible and never mutates the
//! live instance; the framework applies the action infallibly during the terminal
//! publication, and a failed or rejected transition drops it unapplied.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use upwell_core::DependencyDescriptor;

use super::handoff::Handoff;
use super::{FromContainer, Previous};
use crate::descriptors::{BoxedComponent, Component, ComponentConstructionContext};

type PrepareFuture<'a> = Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + 'a>>;

/// An owned in-place update for component `C`, prepared before commit.
///
/// The action receives the retained instance at commit and must not fail; any fallible
/// work belongs in the preparation that returns it.
pub struct HotUpdate<C> {
    apply: Box<dyn FnOnce(&C) + Send>,
}

/// A prepared hot update bound to its retained instance, ready for the terminal
/// publication.
#[doc(hidden)]
pub struct HotUpdateCommit {
    component: &'static str,
    commit: Box<dyn FnOnce() + Send>,
}

/// Normalizes a hot-update preparation's return value: a [`HotUpdate`], or a
/// `Result<HotUpdate, E>`.
pub trait HotUpdateOutput {
    type Component: Component;

    /// Unwraps to the update, mapping any error into [`Error`](crate::Error).
    fn into_update(self) -> crate::Result<HotUpdate<Self::Component>>;
}

/// An async hot-update preparation: [`Previous<C>`] first, then [`FromContainer`]
/// parameters, returning a [`HotUpdateOutput`] for the same component `C`.
///
/// Implemented only for components stored as `Arc<Self>`, the handle that shares the
/// retained instance itself. A by-value handle derives a separate copy on every
/// resolution, so an update applied to it would be lost; such components cannot
/// declare a hot update.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a hot-update preparation",
    label = "expected `async fn(Previous<C>, ..) -> HotUpdate<C>`",
    note = "a hot update takes `Previous<C>` first, then at most seven injectable parameters, and returns `HotUpdate<C>` or `Result<HotUpdate<C>, E>`",
    note = "only components stored as `Arc<Self>` can declare one; a `by_value` component cannot"
)]
pub trait HotUpdateFactory<Args>: Sized {
    /// The dependency edges of the parameters after [`Previous`], in order.
    fn dependencies() -> Vec<DependencyDescriptor>;

    /// Reads the previous instance, resolves every other parameter from `cx`, runs the
    /// preparation, and boxes the commit bound to that instance.
    fn prepare(
        self,
        cx: &mut ComponentConstructionContext,
    ) -> impl Future<Output = crate::Result<BoxedComponent>> + Send;
}

impl<C> HotUpdate<C> {
    /// An update that runs `apply` against the retained instance at commit.
    pub fn new(apply: impl FnOnce(&C) + Send + 'static) -> Self {
        Self {
            apply: Box::new(apply),
        }
    }
}

impl<C: Component> HotUpdateOutput for HotUpdate<C> {
    type Component = C;

    fn into_update(self) -> crate::Result<HotUpdate<C>> {
        Ok(self)
    }
}

impl<C, E> HotUpdateOutput for Result<HotUpdate<C>, E>
where
    C: Component,
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Component = C;

    fn into_update(self) -> crate::Result<HotUpdate<C>> {
        self.map_err(|error| crate::Error::Other(error.into()))
    }
}

impl HotUpdateCommit {
    /// The stable ID of the component this update applies to.
    pub fn component(&self) -> &'static str {
        self.component
    }

    /// Applies the update to its retained instance.
    pub fn commit(self) {
        (self.commit)();
    }

    /// Unboxes a hot-update preparation's commit, or `None` when `boxed` is not one.
    pub(crate) fn take(boxed: BoxedComponent) -> Option<Self> {
        Handoff::take(boxed)
    }
}

impl std::fmt::Debug for HotUpdateCommit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HotUpdateCommit").finish_non_exhaustive()
    }
}

macro_rules! impl_hot_update_factory {
    ( $($ty:ident),* ) => {
        impl<F, Fut, Out, C, $($ty,)*> HotUpdateFactory<(Previous<C>, $($ty,)*)> for F
        where
            F: FnOnce(Previous<C>, $($ty,)*) -> Fut + Send,
            Fut: Future<Output = Out> + Send,
            Out: HotUpdateOutput<Component = C>,
            C: Component<Handle = Arc<C>>,
            $( $ty: FromContainer + Send, )*
        {
            fn dependencies() -> Vec<DependencyDescriptor> {
                vec![ $( <$ty as FromContainer>::dependency() ),* ]
            }

            #[allow(non_snake_case, unused_mut)]
            async fn prepare(
                self,
                cx: &mut ComponentConstructionContext,
            ) -> crate::Result<BoxedComponent> {
                let instance = cx.previous_handle::<C>()?;
                let previous = Previous(cx.previous_handle::<C>()?);
                $( let $ty = <$ty as FromContainer>::from_container(cx).await?; )*

                let update = (self)(previous, $($ty,)*).await.into_update()?;
                let commit = HotUpdateCommit {
                    component: C::ID,
                    commit: Box::new(move || (update.apply)(&instance)),
                };

                Ok(Handoff::boxed(commit))
            }
        }
    };
}

impl_hot_update_factory!();
impl_hot_update_factory!(T1);
impl_hot_update_factory!(T1, T2);
impl_hot_update_factory!(T1, T2, T3);
impl_hot_update_factory!(T1, T2, T3, T4);
impl_hot_update_factory!(T1, T2, T3, T4, T5);
impl_hot_update_factory!(T1, T2, T3, T4, T5, T6);
impl_hot_update_factory!(T1, T2, T3, T4, T5, T6, T7);

/// The dependency edges of a hot-update `factory` after its [`Previous`] parameter.
pub fn hot_update_dependencies<F, Args>(_factory: F) -> Vec<DependencyDescriptor>
where
    F: HotUpdateFactory<Args>,
{
    <F as HotUpdateFactory<Args>>::dependencies()
}

/// Drives a hot-update `factory` to its boxed commit, erased to the
/// [`ComponentFactory`](crate::descriptors::ComponentFactory) fn-pointer shape.
pub fn dispatch_hot_update<'a, F, Args>(
    factory: F,
    cx: &'a mut ComponentConstructionContext,
) -> PrepareFuture<'a>
where
    F: HotUpdateFactory<Args> + 'a,
    Args: 'a,
{
    Box::pin(factory.prepare(cx))
}
