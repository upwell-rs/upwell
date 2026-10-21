//! Static state-aware replacement factories.
//!
//! A replacement factory is an ordinary async factory whose first parameter is
//! [`Previous<C>`]: the immutable handle of the instance the runtime transition replaces.
//! Every further parameter is a [`FromContainer`] resolved from the **candidate**
//! generation, exactly as for an ordinary factory. The previous instance is never
//! handed out mutably and is never consumed, so a later preparation failure leaves the
//! active generation untouched and usable.

use std::future::Future;
use std::ops::Deref;
use std::pin::Pin;

use upwell_core::{DependencyDescriptor, TypeDescriptor};

use super::{FactoryOutput, FromContainer};
use crate::descriptors::{BoxedComponent, Component, ComponentConstructionContext, Injectable};

/// The immutable handle of the active instance a replacement factory replaces.
///
/// For the default `Arc<C>` handle this shares the previous instance, so state carried
/// across a transition is read through shared references (or interior mutability the
/// component already owns). Readers holding the previous instance keep observing it
/// until they drop it.
pub struct Previous<C: Component>(pub(super) C::Handle);

impl<C: Component> Previous<C> {
    /// The previous instance's handle.
    pub fn into_inner(self) -> C::Handle {
        self.0
    }
}

impl<C: Component> Deref for Previous<C> {
    type Target = C::Handle;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// An async replacement factory: [`Previous<C>`] first, then [`FromContainer`]
/// parameters, returning a [`FactoryOutput`] for the same component `C`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a replacement factory",
    label = "expected `async fn(Previous<C>, ..) -> C`",
    note = "a replacement factory takes `Previous<C>` first, then at most seven injectable parameters, and returns `C` or `Result<C, E>`"
)]
pub trait ReplacementFactory<Args>: Sized {
    /// The dependency edges of the parameters after [`Previous`], in order.
    fn dependencies() -> Vec<DependencyDescriptor>;

    /// Reads the previous instance, resolves every other parameter from `cx`, calls the
    /// factory, and boxes the replacement.
    fn construct(
        self,
        cx: &mut ComponentConstructionContext,
    ) -> impl Future<Output = crate::Result<BoxedComponent>> + Send;
}

macro_rules! impl_replacement_factory {
    ( $($ty:ident),* ) => {
        impl<F, Fut, Out, C, $($ty,)*> ReplacementFactory<(Previous<C>, $($ty,)*)> for F
        where
            F: FnOnce(Previous<C>, $($ty,)*) -> Fut + Send,
            Fut: Future<Output = Out> + Send,
            Out: FactoryOutput<Component = C>,
            C: Component,
            $( $ty: FromContainer + Send, )*
        {
            fn dependencies() -> Vec<DependencyDescriptor> {
                vec![ $( <$ty as FromContainer>::dependency() ),* ]
            }

            #[allow(non_snake_case, unused_mut)]
            async fn construct(
                self,
                cx: &mut ComponentConstructionContext,
            ) -> crate::Result<BoxedComponent> {
                let previous = Previous(cx.previous_handle::<C>()?);
                $( let $ty = <$ty as FromContainer>::from_container(cx).await?; )*

                let output = (self)(previous, $($ty,)*).await;
                let component = output.into_component()?;
                let handle = C::into_handle(component);

                Ok(BoxedComponent {
                    ty: TypeDescriptor::of::<C>(C::NAME),
                    value: ::std::boxed::Box::new(Injectable::into_stored(handle)),
                })
            }
        }
    };
}

impl_replacement_factory!();
impl_replacement_factory!(T1);
impl_replacement_factory!(T1, T2);
impl_replacement_factory!(T1, T2, T3);
impl_replacement_factory!(T1, T2, T3, T4);
impl_replacement_factory!(T1, T2, T3, T4, T5);
impl_replacement_factory!(T1, T2, T3, T4, T5, T6);
impl_replacement_factory!(T1, T2, T3, T4, T5, T6, T7);

/// The dependency edges of `factory` after its [`Previous`] parameter.
pub fn replacement_dependencies<F, Args>(_factory: F) -> Vec<DependencyDescriptor>
where
    F: ReplacementFactory<Args>,
{
    <F as ReplacementFactory<Args>>::dependencies()
}

/// Drives a replacement `factory` to a boxed component, erased to the
/// [`ComponentFactory`](crate::descriptors::ComponentFactory) fn-pointer shape.
pub fn dispatch_replacement<'a, F, Args>(
    factory: F,
    cx: &'a mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + 'a>>
where
    F: ReplacementFactory<Args> + 'a,
    Args: 'a,
{
    Box::pin(factory.construct(cx))
}
