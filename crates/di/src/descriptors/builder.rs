//! `const` builders for component and provider descriptors.
//!
//! The descriptors `#[component]` generates can be declared by hand with the same
//! inspectable output: [`ComponentDescriptor::of`] plus the `with_*` setters, the
//! [`factories!`](crate::factories) macro for a component's construction recipes, and
//! [`ProviderDescriptor::of`] for a trait mapping through a [`ProviderOf`] impl.
//!
//! ```
//! use std::sync::Arc;
//!
//! use upwell_di::{Component, ComponentDescriptor, ProviderDescriptor, ProviderOf, factories};
//!
//! trait Greeter: Send + Sync {}
//!
//! struct English;
//!
//! impl Greeter for English {}
//!
//! impl Component for English {
//!     type Handle = Arc<Self>;
//!
//!     const ID: &'static str = "english";
//!     const NAME: &'static str = "English";
//!
//!     fn into_handle(self) -> Self::Handle {
//!         Arc::new(self)
//!     }
//! }
//!
//! impl ProviderOf<dyn Greeter> for English {
//!     fn upcast(this: Arc<Self>) -> Arc<dyn Greeter> {
//!         this
//!     }
//! }
//!
//! async fn english() -> English {
//!     English
//! }
//!
//! static ENGLISH: ComponentDescriptor =
//!     ComponentDescriptor::of::<English>().with_factories(factories![english]);
//! static ENGLISH_GREETER: ProviderDescriptor =
//!     ProviderDescriptor::of::<English, dyn Greeter>("dyn Greeter").primary();
//! ```

use std::any::type_name;
use std::sync::Arc;

use upwell_core::{ConditionDescriptor, Scope, TypeDescriptor};
use upwell_hooks::HookDescriptor;

use super::component::{
    BoxedComponent, Component, ComponentDescriptor, ComponentFactoryDescriptor, Injectable, Live,
    ProviderDescriptor, ProviderOf, ProviderOrder,
};

/// Declares a component's construction recipes as the `fn() -> &'static [..]` a
/// [`ComponentDescriptor`] stores, from async factory functions whose parameters are
/// all [`FromContainer`](crate::FromContainer) (any [`Factory`](crate::Factory)).
///
/// Each entry is `path` (its ID is the path as written) or `"id" => path`. A single
/// recipe is the effective one; several need exactly one explicit recipe, as with
/// `#[init]`, so mark the fallback `default "id" => path`.
#[macro_export]
macro_rules! factories {
    ($($entry:tt)*) => {
        || const { &$crate::__factory_entries!([] $($entry)*) }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __factory_entries {
    ([$($done:expr,)*]) => {
        [$($done),*]
    };
    ([$($done:expr,)*] default $id:literal => $factory:path $(, $($rest:tt)*)?) => {
        $crate::__factory_entries!(
            [$($done,)* $crate::__factory_entry!($id, $factory, true),] $($($rest)*)?
        )
    };
    ([$($done:expr,)*] $id:literal => $factory:path $(, $($rest:tt)*)?) => {
        $crate::__factory_entries!(
            [$($done,)* $crate::__factory_entry!($id, $factory, false),] $($($rest)*)?
        )
    };
    ([$($done:expr,)*] $factory:path $(, $($rest:tt)*)?) => {
        $crate::__factory_entries!(
            [$($done,)* $crate::__factory_entry!(stringify!($factory), $factory, false),]
            $($($rest)*)?
        )
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __factory_entry {
    ($id:expr, $factory:path, $default:literal) => {
        $crate::ComponentFactoryDescriptor {
            id: $id,
            construct: |cx| $crate::dispatch_factory($factory, cx),
            dependencies: || $crate::factory_dependencies($factory),
            default: $default,
        }
    };
}

impl ComponentDescriptor {
    /// Replaces the construction recipes, typically with [`factories!`](crate::factories).
    pub const fn with_factories(
        self,
        factories: fn() -> &'static [ComponentFactoryDescriptor],
    ) -> Self {
        Self { factories, ..self }
    }

    /// Makes the component eligible only while `condition` holds.
    pub const fn with_condition(self, condition: &'static ConditionDescriptor) -> Self {
        Self {
            condition: Some(condition),
            ..self
        }
    }

    /// Attaches the component's hook methods.
    pub const fn with_hooks(self, hooks: fn() -> &'static [HookDescriptor]) -> Self {
        Self { hooks, ..self }
    }

    /// Places the component in `scope` instead of the singleton scope.
    pub const fn with_scope(self, scope: &'static dyn Scope) -> Self {
        Self { scope, ..self }
    }

    /// Drops the typed generation snapshot, so a transition never retains the instance
    /// across generations and always rebuilds it.
    pub const fn without_retention(self) -> Self {
        Self {
            generation_snapshot: None,
            ..self
        }
    }
}

impl ProviderDescriptor {
    /// Maps component `T` to the trait object `P` it provides, under the qualifier
    /// `T::ID`, non-primary, with priority `0` and no ordering constraints.
    /// `trait_name` is the trait object as written (`"dyn Trait"`).
    pub const fn of<T, P>(trait_name: &'static str) -> Self
    where
        T: Component + ProviderOf<P>,
        P: ?Sized + Send + Sync + 'static,
    {
        Self {
            trait_ty: TypeDescriptor::of::<P>(trait_name),
            concrete_ty: TypeDescriptor::of::<T>(T::NAME),
            qualifier: T::ID,
            primary: false,
            priority: 0,
            ordering: &[],
            erase: erase_provider::<T, P>,
        }
    }

    /// Makes this the provider chosen for a single `Arc<dyn Trait>` dependency.
    pub const fn primary(self) -> Self {
        Self {
            primary: true,
            ..self
        }
    }

    /// Keys this provider by `qualifier` in a `HashMap<String, Arc<dyn Trait>>`.
    pub const fn qualifier(self, qualifier: &'static str) -> Self {
        Self { qualifier, ..self }
    }

    /// Sets the collection-order priority; lower values come first.
    pub const fn priority(self, priority: i64) -> Self {
        Self { priority, ..self }
    }

    /// Adds relative ordering constraints against other providers.
    pub const fn ordering(self, ordering: &'static [ProviderOrder]) -> Self {
        Self { ordering, ..self }
    }
}

impl BoxedComponent {
    /// Boxes a constructed component in its storage handle, as factories return it.
    pub fn new<T: Component>(component: T) -> Self {
        Self {
            ty: TypeDescriptor::of::<T>(T::NAME),
            value: Box::new(Injectable::into_stored(T::into_handle(component))),
        }
    }
}

/// Re-erases a built `T` as the trait object `P` for storage under the trait's key.
/// The [`ProviderDescriptor::erase`] every provider of `P` declared through
/// [`ProviderDescriptor::of`] or `provide = dyn Trait` uses.
pub fn erase_provider<T, P>(concrete: &BoxedComponent) -> BoxedComponent
where
    T: Component + ProviderOf<P>,
    P: ?Sized + Send + Sync + 'static,
{
    let shared = concrete
        .value
        .downcast_ref::<Live<T>>()
        .expect("provider concrete type mismatch")
        .snapshot();
    let erased: Arc<P> = T::upcast(shared);

    BoxedComponent {
        ty: TypeDescriptor::of::<P>(type_name::<P>()),
        value: Box::new(Live::new(erased)),
    }
}

#[cfg(test)]
mod tests;
