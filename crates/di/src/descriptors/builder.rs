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
    LiveCell, ProviderDescriptor, ProviderOf, ProviderOrder,
};

/// Declares a component's construction recipes as the `fn() -> &'static [..]` a
/// [`ComponentDescriptor`] stores, from async factory functions whose parameters are
/// all [`FromContainer`](crate::FromContainer) (any [`Factory`](crate::Factory)).
///
/// Each entry is `path` or `"id" => path`. A bare `path` is identified as
/// `"factory:<path>"`, the ID `factory = path` generates. A single recipe is the
/// effective one; several need exactly one explicit recipe, as with `#[init]`, so mark
/// the one fallback `default "id" => path`. A static state-aware replacement, whose
/// first parameter is [`Previous<Self>`](crate::Previous), is declared as
/// `replace "id" => path`; it never takes part in the ordinary choice. A transition
/// handler, a plain `fn(&ComponentChange) -> Option<OneShot<Self>>`, is declared as
/// `transition "id" => path`, and an explicit hot-update preparation, whose first
/// parameter is `Previous<Self>` and which returns a [`HotUpdate`](crate::HotUpdate), is
/// declared as `hot_update "id" => path`. A second `default` is a compile error:
///
/// ```compile_fail
/// # use std::sync::Arc;
/// # struct Plain;
/// # impl upwell_di::Component for Plain {
/// #     type Handle = Arc<Self>;
/// #     const ID: &'static str = "plain";
/// #     const NAME: &'static str = "Plain";
/// #     fn into_handle(self) -> Arc<Self> { Arc::new(self) }
/// # }
/// async fn first() -> Plain { Plain }
/// async fn second() -> Plain { Plain }
///
/// let _ = upwell_di::factories![default "first" => first, default "second" => second];
/// ```
#[macro_export]
macro_rules! factories {
    ($($entry:tt)*) => {
        || const { &$crate::__factory_entries!([] no $($entry)*) }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __factory_entries {
    ([$($done:expr,)*] $default:ident) => {
        [$($done),*]
    };
    ([$($done:expr,)*] yes default $id:literal => $factory:path $(, $($rest:tt)*)?) => {
        ::core::compile_error!("factories! declares more than one `default` recipe")
    };
    ([$($done:expr,)*] no default $id:literal => $factory:path $(, $($rest:tt)*)?) => {
        $crate::__factory_entries!(
            [$($done,)* $crate::__factory_entry!($id, $factory, Default),] yes $($($rest)*)?
        )
    };
    ([$($done:expr,)*] $default:ident replace $id:literal => $factory:path $(, $($rest:tt)*)?) => {
        $crate::__factory_entries!(
            [$($done,)* $crate::__replacement_entry!($id, $factory),] $default $($($rest)*)?
        )
    };
    ([$($done:expr,)*] $default:ident transition $id:literal => $handler:path $(, $($rest:tt)*)?) => {
        $crate::__factory_entries!(
            [$($done,)* $crate::__transition_entry!($id, $handler),] $default $($($rest)*)?
        )
    };
    ([$($done:expr,)*] $default:ident hot_update $id:literal => $factory:path $(, $($rest:tt)*)?) => {
        $crate::__factory_entries!(
            [$($done,)* $crate::__hot_update_entry!($id, $factory),] $default $($($rest)*)?
        )
    };
    ([$($done:expr,)*] $default:ident $id:literal => $factory:path $(, $($rest:tt)*)?) => {
        $crate::__factory_entries!(
            [$($done,)* $crate::__factory_entry!($id, $factory, Explicit),] $default
            $($($rest)*)?
        )
    };
    ([$($done:expr,)*] $default:ident $factory:path $(, $($rest:tt)*)?) => {
        $crate::__factory_entries!(
            [
                $($done,)*
                $crate::__factory_entry!(
                    ::core::concat!("factory:", ::core::stringify!($factory)),
                    $factory,
                    Explicit
                ),
            ]
            $default $($($rest)*)?
        )
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __factory_entry {
    ($id:expr, $factory:path, $role:ident) => {
        $crate::ComponentFactoryDescriptor {
            id: $id,
            construct: |cx| $crate::dispatch_factory($factory, cx),
            dependencies: || $crate::factory_dependencies($factory),
            role: $crate::ComponentFactoryRole::$role,
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __replacement_entry {
    ($id:expr, $factory:path) => {
        $crate::ComponentFactoryDescriptor {
            id: $id,
            construct: |cx| $crate::dispatch_replacement($factory, cx),
            dependencies: || $crate::replacement_dependencies($factory),
            role: $crate::ComponentFactoryRole::Replacement,
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __transition_entry {
    ($id:expr, $handler:path) => {
        $crate::ComponentFactoryDescriptor {
            id: $id,
            construct: |cx| $crate::dispatch_transition($handler, cx),
            dependencies: ::std::vec::Vec::new,
            role: $crate::ComponentFactoryRole::Transition,
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __hot_update_entry {
    ($id:expr, $factory:path) => {
        $crate::ComponentFactoryDescriptor {
            id: $id,
            construct: |cx| $crate::dispatch_hot_update($factory, cx),
            dependencies: || $crate::hot_update_dependencies($factory),
            role: $crate::ComponentFactoryRole::HotUpdate,
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

    /// Gives the component the stable ID `id` and the name `name` in place of its
    /// [`Component`] constants, for framework-seeded instances with a custom identity.
    /// The type descriptor is renamed too, so `ty.name` becomes `name` rather than
    /// `T::NAME`.
    pub const fn with_identity(self, id: &'static str, name: &'static str) -> Self {
        Self {
            id,
            name,
            ty: TypeDescriptor { name, ..self.ty },
            ..self
        }
    }

    /// Places the component in `scope` instead of the singleton scope.
    pub const fn with_scope(self, scope: &'static dyn Scope) -> Self {
        Self { scope, ..self }
    }

    /// Opts out of retention, so a transition never carries the instance across
    /// generations and always rebuilds it. Live consumers of the component can still be
    /// rebound in place, because its typed live-cell access is kept.
    pub const fn without_retention(self) -> Self {
        let generation_snapshot = match self.generation_snapshot {
            Some(adapter) => Some(adapter.without_retention()),
            None => None,
        };

        Self {
            generation_snapshot,
            ..self
        }
    }
}

impl ProviderDescriptor {
    /// Maps component `T` to the trait object `P` it provides, under the qualifier
    /// `T::ID`, non-primary, with priority `0` and no ordering constraints.
    /// `trait_name` is the trait object as written (`"dyn Trait"`). `T` must be stored in
    /// a [`Live<T>`] cell (an `Arc<T>` or `Dep<T>` handle), so a by-value component cannot
    /// be mapped to a trait:
    ///
    /// ```compile_fail,E0271
    /// # use std::sync::Arc;
    /// # use upwell_di::{Component, Injectable, ProviderDescriptor, ProviderOf};
    /// trait Greeter: Send + Sync {}
    ///
    /// #[derive(Clone)]
    /// struct ByValue;
    ///
    /// impl Greeter for ByValue {}
    ///
    /// impl Component for ByValue {
    ///     type Handle = Self;
    ///
    ///     const ID: &'static str = "by-value";
    ///     const NAME: &'static str = "ByValue";
    ///
    ///     fn into_handle(self) -> Self {
    ///         self
    ///     }
    /// }
    ///
    /// impl Injectable for ByValue {
    ///     type Target = Self;
    ///     type Stored = Self;
    ///
    ///     fn into_stored(self) -> Self {
    ///         self
    ///     }
    ///
    ///     fn from_stored(stored: &Self) -> Self {
    ///         stored.clone()
    ///     }
    /// }
    ///
    /// impl ProviderOf<dyn Greeter> for ByValue {
    ///     fn upcast(this: Arc<Self>) -> Arc<dyn Greeter> {
    ///         this
    ///     }
    /// }
    ///
    /// static GREETER: ProviderDescriptor = ProviderDescriptor::of::<ByValue, dyn Greeter>("dyn Greeter");
    /// ```
    pub const fn of<T, P>(trait_name: &'static str) -> Self
    where
        T: Component + ProviderOf<P>,
        T::Handle: Injectable<Stored = Live<T>>,
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
            live: provider_live_cell::<P>,
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
///
/// The `Live<T>` bound on `T`'s handle makes the downcast hold by construction: the
/// container only erases the box `T`'s own descriptor built, and
/// [`BoxedComponent::new`] stores `T` as its handle's `Stored` type. A mismatch is a
/// container bug, so it panics rather than surfacing as a recoverable error.
pub fn erase_provider<T, P>(concrete: &BoxedComponent) -> BoxedComponent
where
    T: Component + ProviderOf<P>,
    T::Handle: Injectable<Stored = Live<T>>,
    P: ?Sized + Send + Sync + 'static,
{
    let shared = concrete
        .value
        .downcast_ref::<Live<T>>()
        .expect("the container erases only the concrete box the provider's own descriptor built")
        .snapshot();
    let erased: Arc<P> = T::upcast(shared);

    BoxedComponent {
        ty: TypeDescriptor::of::<P>(type_name::<P>()),
        value: Box::new(Live::new(erased)),
    }
}

/// The live cell an erased trait slot for `P` holds: the
/// [`ProviderDescriptor::live`] accessor every [`ProviderDescriptor::of`] mapping uses.
#[doc(hidden)]
pub fn provider_live_cell<P>(erased: &BoxedComponent) -> Option<LiveCell>
where
    P: ?Sized + Send + Sync + 'static,
{
    erased.value.downcast_ref::<Live<P>>().map(LiveCell::of)
}

#[cfg(test)]
mod tests;
