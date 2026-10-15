//! Construction-time dependency injection: the build-time analogue of the
//! request-time extract layer.
//!
//! A **factory** is a function the user writes (an `#[init]` constructor, or a
//! `factory = ..` path) whose parameters are dependencies and whose return value is
//! the component. [`Factory`] is implemented for every such function (keyed on its
//! argument tuple): it knows its parameters, so it reports its [`DependencyDescriptor`]s
//! and resolves each one from the [`ComponentConstructionContext`] before calling the
//! user's function. [`FromContainer`] is the per-argument extractor it builds on;
//! [`FactoryOutput`] normalizes the return value (a component, or a `Result<component, E>`).
//!
//! Dependencies are reported at runtime (`fn() -> Vec<..>`) rather than as a `const`,
//! because building a descriptor needs the type's name and `type_name` is not yet
//! const-stable; the descriptors are read only at build time, so the cost is nil.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use upwell_core::{
    Cardinality, DependencyDescriptor, DependencyObservation, ResolutionMode, TypeDescriptor,
};

use crate::descriptors::{
    BoxedComponent, Component, ComponentConstructionContext, Dep, Injectable,
};
use crate::error::Error;

/// The short, human-readable type name (the final `::` segment of the full path),
/// borrowed from the `'static` full name — so it allocates nothing.
pub fn short_name<T: ?Sized + 'static>() -> &'static str {
    let full = std::any::type_name::<T>();

    full.rsplit("::").next().unwrap_or(full)
}

/// Builds a dependency edge for type `T`.
pub fn dependency_of<T: ?Sized + 'static>(
    cardinality: Cardinality,
    optional: bool,
    config: bool,
) -> DependencyDescriptor {
    dependency_of_observed::<T>(
        cardinality,
        optional,
        config,
        DependencyObservation::Snapshot,
    )
}

/// Builds a dependency edge for type `T` with an explicit observation contract.
pub fn dependency_of_observed<T: ?Sized + 'static>(
    cardinality: Cardinality,
    optional: bool,
    config: bool,
    observation: DependencyObservation,
) -> DependencyDescriptor {
    DependencyDescriptor {
        name: short_name::<T>(),
        ty: TypeDescriptor::of::<T>(short_name::<T>()),
        cardinality,
        optional,
        dynamic: false,
        qualifier: None,
        config,
        resolution: ResolutionMode::Eager,
        observation,
    }
}

/// A single factory parameter, resolvable from the construction context, that
/// reports the dependency edge it represents.
///
/// Implemented for the injectable parameter shapes — `Arc<T>` (a single component
/// or the primary/sole `Arc<dyn Trait>`), a by-value injectable (`Dir<K>`,
/// `PeerInfo`, a `#[component(by_value)]` type), `Option<Arc<T>>` (an optional
/// component), `Vec<Arc<dyn Trait>>` (every provider), and
/// `HashMap<String, Arc<dyn Trait>>` (providers keyed by qualifier). The config layer
/// adds an impl for `Cfg<T>` in its own crate (resolving via the config resolver).
pub trait FromContainer: Sized {
    /// The dependency edge this parameter contributes, for validation and ordering.
    fn dependency() -> DependencyDescriptor;

    /// Resolves this parameter from the construction context.
    fn from_container(
        cx: &ComponentConstructionContext,
    ) -> impl Future<Output = crate::Result<Self>> + Send;
}

impl<T> FromContainer for Arc<T>
where
    T: ?Sized + Send + Sync + 'static,
{
    fn dependency() -> DependencyDescriptor {
        dependency_of::<T>(Cardinality::One, false, false)
    }

    async fn from_container(cx: &ComponentConstructionContext) -> crate::Result<Self> {
        cx.resolve::<Arc<T>>()
            .await?
            .ok_or(Error::MissingComponent(short_name::<T>()))
    }
}

/// A live, reloadable dependency. Resolves the same component as `Arc<T>` (keyed by
/// `T`), but hands back a `Dep<T>` sharing the component's live slot, so a later
/// reload swap is observed. `Target = T ≠ Dep<T>`, so this does not overlap the
/// by-value blanket below.
impl<T> FromContainer for Dep<T>
where
    T: ?Sized + Send + Sync + 'static,
{
    fn dependency() -> DependencyDescriptor {
        dependency_of_observed::<T>(Cardinality::One, false, false, DependencyObservation::Live)
    }

    async fn from_container(cx: &ComponentConstructionContext) -> crate::Result<Self> {
        cx.resolve::<Dep<T>>()
            .await?
            .ok_or(Error::MissingComponent(short_name::<T>()))
    }
}

/// By-value injectables — those whose injectable handle *is* the type itself
/// (`Injectable<Target = Self>`): `Dir<K>`, `PeerInfo`, `ShutdownHandle`, and any
/// `#[component(by_value)]` type. The `Target = Self` bound excludes `Arc<T>`
/// (`Target = T`) and `Cfg<T>` (`Target = T`), so this does not overlap their
/// dedicated impls.
impl<H> FromContainer for H
where
    H: Injectable<Target = H> + Send + Sync + 'static,
{
    fn dependency() -> DependencyDescriptor {
        dependency_of::<H>(Cardinality::One, false, false)
    }

    async fn from_container(cx: &ComponentConstructionContext) -> crate::Result<Self> {
        cx.resolve::<H>()
            .await?
            .ok_or(Error::MissingComponent(short_name::<H>()))
    }
}

impl<T> FromContainer for Option<Arc<T>>
where
    T: ?Sized + Send + Sync + 'static,
{
    fn dependency() -> DependencyDescriptor {
        dependency_of::<T>(Cardinality::One, true, false)
    }

    async fn from_container(cx: &ComponentConstructionContext) -> crate::Result<Self> {
        cx.resolve::<Arc<T>>().await
    }
}

impl<T> FromContainer for Vec<Arc<T>>
where
    T: ?Sized + Send + Sync + 'static,
{
    fn dependency() -> DependencyDescriptor {
        dependency_of::<T>(Cardinality::Collection, false, false)
    }

    async fn from_container(cx: &ComponentConstructionContext) -> crate::Result<Self> {
        cx.resolve_all::<Arc<T>>().await
    }
}

impl<T> FromContainer for HashMap<String, Arc<T>>
where
    T: ?Sized + Send + Sync + 'static,
{
    fn dependency() -> DependencyDescriptor {
        dependency_of::<T>(Cardinality::Keyed, false, false)
    }

    async fn from_container(cx: &ComponentConstructionContext) -> crate::Result<Self> {
        cx.resolve_keyed::<Arc<T>>().await
    }
}

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

macro_rules! impl_factory {
    ( $($ty:ident),* ) => {
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
                $( let $ty = <$ty as FromContainer>::from_container(cx).await?; )*

                let output = (self)($($ty,)*).await;
                let component = output.into_component()?;

                let handle = <Out::Component as Component>::into_handle(component);

                Ok(BoxedComponent {
                    ty: TypeDescriptor::of::<Out::Component>(
                        <Out::Component as Component>::NAME,
                    ),
                    value: ::std::boxed::Box::new(Injectable::into_stored(handle)),
                })
            }
        }
    };
}

impl_factory!();
impl_factory!(T1);
impl_factory!(T1, T2);
impl_factory!(T1, T2, T3);
impl_factory!(T1, T2, T3, T4);
impl_factory!(T1, T2, T3, T4, T5);
impl_factory!(T1, T2, T3, T4, T5, T6);
impl_factory!(T1, T2, T3, T4, T5, T6, T7);
impl_factory!(T1, T2, T3, T4, T5, T6, T7, T8);

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
