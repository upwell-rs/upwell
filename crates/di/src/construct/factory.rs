//! The [`Factory`] and [`ReadyFactory`] impls that turn a user function into a construction
//! recipe, and the dispatchers that erase them to descriptor fn pointers.

use std::future::Future;
use std::pin::Pin;

use upwell_core::{DependencyDescriptor, TypeDescriptor};

use super::FromContainer;
use crate::descriptors::{BoxedComponent, Component, ComponentConstructionContext};
use crate::error::Error;

/// What a factory function may return: the component itself, or a `Result` of it.
///
/// The `Component` blanket and the `Result` impl coexist because `Component` is a
/// local trait no `Result` implements (and the orphan rule forbids one downstream),
/// so the compiler can prove they do not overlap.
pub trait FactoryOutput {
    type Component: Component;

    /// Unwraps to the constructed component, mapping any error into [`Error`].
    fn into_component(self) -> crate::Result<Self::Component>;
}

impl<C: Component> FactoryOutput for C {
    type Component = C;

    fn into_component(self) -> crate::Result<C> {
        Ok(self)
    }
}

impl<C, E> FactoryOutput for Result<C, E>
where
    C: Component,
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Component = C;

    fn into_component(self) -> crate::Result<C> {
        // Funnel any error into the DI error's `Other` arm. The factory error channel sits
        // at the DI layer, below the daemon, so a constructor returning a higher-layer or
        // domain error (`upwell::Result`, an app error) cannot convert *into* `di::Error`
        // directly. `Into<Box<dyn Error + Send + Sync>>` is the common denominator: every
        // `Error + Send + Sync` type satisfies it, and so does `Box<dyn Error + Send + Sync>`
        // itself (the boxed-error constructor case).
        self.map_err(|e| Error::Other(e.into()))
    }
}

/// An async factory function whose parameters are all [`FromContainer`] and whose
/// return value is a [`FactoryOutput`]. `Args` is the parameter-tuple marker that
/// selects the arity impl.
///
/// Every user-provided factory is a `Factory`: it reports its [`dependencies`] from
/// its parameters and [`construct`]s by resolving each, calling the function, and
/// boxing the component.
///
/// [`dependencies`]: Factory::dependencies
/// [`construct`]: Factory::construct
pub trait Factory<Args>: Sized {
    /// The dependency edges of this factory's parameters, in order.
    fn dependencies() -> Vec<DependencyDescriptor>;

    /// Resolves every parameter from `cx`, calls the factory, and boxes the result.
    fn construct(
        self,
        cx: &mut ComponentConstructionContext,
    ) -> impl Future<Output = crate::Result<BoxedComponent>> + Send;
}

/// A synchronous factory function whose parameters are all [`FromContainer`], built without
/// a future when every parameter is ready to read.
///
/// `#[init]` uses it for a constructor that is not `async`, beside the async [`Factory`]
/// that remains its fallback.
#[doc(hidden)]
pub trait ReadyFactory<Args>: Sized {
    /// Reads every parameter from `cx` and calls the factory, or returns `Ok(None)` when a
    /// parameter is not ready to read.
    fn construct_ready(
        self,
        cx: &ComponentConstructionContext,
    ) -> crate::Result<Option<BoxedComponent>>;
}

macro_rules! impl_factory {
    ( $($ty:ident $index:literal),* ) => {
        impl<F, Fut, Out, $($ty,)*> Factory<($($ty,)*)> for F
        where
            F: FnOnce($($ty,)*) -> Fut + Send,
            Fut: Future<Output = Out> + Send,
            Out: FactoryOutput,
            $( $ty: FromContainer + Send, )*
        {
            fn dependencies() -> Vec<DependencyDescriptor> {
                vec![ $( <$ty as FromContainer>::dependency() ),* ]
            }

            #[allow(non_snake_case, unused_variables, unused_mut)]
            async fn construct(
                self,
                cx: &mut ComponentConstructionContext,
            ) -> crate::Result<BoxedComponent> {
                $( let $ty = <$ty as FromContainer>::from_container_at(cx, $index).await?; )*

                into_boxed(cx, (self)($($ty,)*).await)
            }
        }

        impl<F, Out, $($ty,)*> ReadyFactory<($($ty,)*)> for F
        where
            F: FnOnce($($ty,)*) -> Out,
            Out: FactoryOutput,
            $( $ty: FromContainer, )*
        {
            #[allow(non_snake_case, unused_variables)]
            fn construct_ready(
                self,
                cx: &ComponentConstructionContext,
            ) -> crate::Result<Option<BoxedComponent>> {
                $(
                    let Some($ty) = <$ty as FromContainer>::ready_at(cx, $index) else {
                        return Ok(None);
                    };
                    let $ty = $ty?;
                )*

                into_boxed(cx, (self)($($ty,)*)).map(Some)
            }
        }
    };
}

impl_factory!();
impl_factory!(T1 0);
impl_factory!(T1 0, T2 1);
impl_factory!(T1 0, T2 1, T3 2);
impl_factory!(T1 0, T2 1, T3 2, T4 3);
impl_factory!(T1 0, T2 1, T3 2, T4 3, T5 4);
impl_factory!(T1 0, T2 1, T3 2, T4 3, T5 4, T6 5);
impl_factory!(T1 0, T2 1, T3 2, T4 3, T5 4, T6 5, T7 6);
impl_factory!(T1 0, T2 1, T3 2, T4 3, T5 4, T6 5, T7 6, T8 7);

/// Unwraps a factory's output and boxes its component's handle as `cx`'s scope stores it.
fn into_boxed<Out: FactoryOutput>(
    cx: &ComponentConstructionContext,
    output: Out,
) -> crate::Result<BoxedComponent> {
    let component = output.into_component()?;
    let handle = <Out::Component as Component>::into_handle(component);

    Ok(BoxedComponent {
        ty: TypeDescriptor::of::<Out::Component>(<Out::Component as Component>::NAME),
        value: Box::new(cx.stored(handle)),
    })
}

/// The dependency edges of `factory`, recovered from its [`Factory`] impl. The
/// factory value is consumed only to infer `Args`; the macro wraps a non-capturing
/// call to this in the `fn() -> Vec<..>` the descriptor stores.
pub fn factory_dependencies<F, Args>(_factory: F) -> Vec<DependencyDescriptor>
where
    F: Factory<Args>,
{
    <F as Factory<Args>>::dependencies()
}

/// Drives `factory` to a boxed component, erased to the
/// [`ComponentFactory`](crate::descriptors::ComponentFactory) fn-pointer shape the
/// descriptor stores. The macro wraps a non-capturing call to this per factory.
pub fn dispatch_factory<'a, F, Args>(
    factory: F,
    cx: &'a mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + 'a>>
where
    F: Factory<Args> + 'a,
    Args: 'a,
{
    Box::pin(factory.construct(cx))
}

/// Builds `factory`'s component without a future when every parameter is ready, erased to
/// the [`ReadyComponentFactory`](crate::descriptors::ReadyComponentFactory) fn-pointer shape.
/// The macro wraps a non-capturing call to this per synchronous `#[init]`.
#[doc(hidden)]
pub fn dispatch_ready_factory<F, Args>(
    factory: F,
    cx: &ComponentConstructionContext,
) -> crate::Result<Option<BoxedComponent>>
where
    F: ReadyFactory<Args>,
{
    factory.construct_ready(cx)
}
