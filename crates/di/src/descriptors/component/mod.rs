use std::ops::Deref;
use std::{any::Any, fmt, sync::Arc};

use upwell_core::{
    ConditionDescriptor, DependencyDescriptor, Scope, ScopeKey, Singleton, TypeDescriptor,
};
use upwell_hooks::{HookDescriptor, no_hooks};

mod context;
mod factory;
mod live;
mod planned;
mod provider;
mod resolve;
mod snapshot;
mod store;

pub use context::ComponentConstructionContext;
pub(crate) use context::{ComponentConstructionParts, HandleStorage, SharedStore};
pub use factory::{
    ComponentFactories, ComponentFactory, ComponentFactoryDescriptor, ComponentFactoryRole,
    DeclaredScope, ReadyComponentFactory, Registration,
};
pub use live::{Dep, Live, LiveCell, LiveRebind, LiveRef};
pub(crate) use planned::Running;
pub use provider::{ProviderDescriptor, ProviderOrder, ProviderOrderDirection};
pub use snapshot::GenerationSnapshot;
pub(crate) use store::ScopeStore;
pub use store::{BoxedComponent, from_boxed};

/// Metadata trait for types registerable as components.
///
/// Supplies the runtime identity used to synthesize a descriptor for a
/// manually-provided instance. Implemented by `#[component]` and `#[service]`.
pub trait Component: Any + Send + Sync + 'static {
    /// The cloneable handle this component is stored in the container as, and
    /// injected by. `Arc<Self>` by default (auto-wrapped); a type that manages
    /// its own sharing — typically because it is internally `Arc` and cheap to
    /// clone — may set this to `Self` via `#[component(by_value)]`, so it is
    /// stored without an extra `Arc`.
    type Handle: Injectable<Target = Self>;

    const ID: &'static str;
    const NAME: &'static str;

    /// Wraps a freshly constructed instance into its storage handle.
    fn into_handle(self) -> Self::Handle;
}

/// A [`Component`] that is also a service, carrying its version in the type.
///
/// Implemented by `#[service]`. Tracking the version on the type (rather than
/// only in the `ServiceDescriptor`) keeps it available generically and opens
/// the door to manual service registration.
pub trait ServiceComponent: Component {
    const VERSION: Option<&'static str>;
}

/// How a resolved dependency is stored in, and handed out of, the container.
///
/// Every injectable *handle* implements this. `Target` is the type the instance
/// is keyed under; cloning a handle happens on every resolution, so it must be
/// cheap.
///
/// A scope does not box the handle directly — it boxes [`Stored`](Self::Stored),
/// the *slot* a handle is derived from. For `Arc<T>` and `Dep<T>` the slot is a
/// shared [`Live<T>`]: an `Arc<T>` snapshots it (one fixed instance), a `Dep<T>`
/// clones it (so a later swap is observed), and both find the *same* slot because
/// they share `Target = T`. A by-value handle is its own slot (`Stored = Self`),
/// so the round-trip is a plain clone. The container converts between slot and
/// handle through [`into_stored`](Self::into_stored) (once, at construction) and
/// [`from_stored`](Self::from_stored) (on every resolution).
pub trait Injectable: Clone + Send + Sync + 'static {
    /// The type this handle is stored and looked up under. `?Sized` so a trait
    /// object (`dyn Trait + Send + Sync`) can key its providers.
    type Target: ?Sized + 'static;

    /// The value actually held in a scope's box for this handle. `Arc<T>` and
    /// `Dep<T>` both store a shared `Live<T>`, so they alias one swappable slot;
    /// by-value handles store themselves.
    type Stored: Clone + Send + Sync + 'static;

    /// Wraps a freshly built handle into its stored slot. Called once, when the
    /// instance is first inserted into a scope.
    fn into_stored(self) -> Self::Stored;

    /// Wraps a freshly built handle stored in a scope below the root. No transition
    /// rebinds such an instance, so a handle may store it without a swappable cell.
    /// Defaults to [`into_stored`](Self::into_stored).
    #[doc(hidden)]
    fn into_fixed(self) -> Self::Stored {
        self.into_stored()
    }

    /// Derives this handle from a stored slot. Called on every resolution — a
    /// snapshot for `Arc<T>`, a shared clone for `Dep<T>`, a plain clone otherwise.
    fn from_stored(stored: &Self::Stored) -> Self;

    /// Creates storage for the same component instance in another runtime generation.
    ///
    /// The default rejects retention. Handles opt in only when they can guarantee that
    /// the returned storage does not share generation-local mutable binding state with
    /// `stored`. `Arc<T>` creates a fresh [`Live<T>`] cell around the current snapshot;
    /// `Dep<T>` deliberately keeps the default because cloning it would share the cell.
    #[doc(hidden)]
    fn snapshot_stored(_: &Self::Stored) -> Option<Self::Stored> {
        None
    }

    /// The shared live cell behind `stored`, when this handle stores one.
    ///
    /// `Arc<T>` and `Dep<T>` store a [`Live<T>`] and return a handle onto that exact
    /// cell, so a runtime transition can republish it for the consumers holding it.
    /// By-value handles store no cell and keep the default.
    #[doc(hidden)]
    fn live_cell(_: &Self::Stored) -> Option<LiveCell> {
        None
    }
}

/// Field wrapper marking a dependency as **runtime-provided**.
///
/// A `Dynamic<H>` dependency is satisfied by a provider registered at runtime
/// rather than one discovered at build, so the edge is exempt from static
/// dependency validation. It wraps the resolved handle `H` and derefs to it for
/// transparent access.
#[derive(Clone)]
pub struct Dynamic<H: Injectable>(pub H);

impl<H: Injectable> Deref for Dynamic<H> {
    type Target = H;

    fn deref(&self) -> &H {
        &self.0
    }
}

impl<H: Injectable> AsRef<H> for Dynamic<H> {
    fn as_ref(&self) -> &H {
        &self.0
    }
}

/// Compile-time dependency marker: `Wiring: Provided<T>` holds when some component
/// in scope provides `T`. Under the `di-check` feature, the macros emit
/// `impl Provided<Self> for Wiring` for every component and a bound per concrete
/// dependency, so a missing provider is a `cargo check` error.
#[diagnostic::on_unimplemented(
    message = "no component provides `{T}`",
    note = "add a component for `{T}`, or mark the dependency `Dynamic<{T}>` if it is provided at runtime"
)]
pub trait Provided<T: ?Sized> {}

/// A concrete component that provides the trait object `T`.
///
/// Generated by `provide = dyn Trait`; implement it by hand to declare a provider
/// without the macros (see [`ProviderDescriptor::of`]). Also bounds explicitly
/// trait-restricted provider ordering.
pub trait ProviderOf<T: ?Sized> {
    /// Unsizes the shared concrete instance into the provided trait object.
    fn upcast(this: Arc<Self>) -> Arc<T>;
}

/// The crate-wide anchor that each component's `Provided` impl attaches to. A
/// dependency assertion is the bound `Wiring: Provided<Dep>`.
pub struct Wiring;

/// Marker that all of a component's dependencies are provided. Under `di-check`
/// the macros emit `impl Wired for T where Wiring: Provided<Dep>, ..` carrying
/// *every* single dependency (concrete and trait-object) as a lazy bound.
pub trait Wired {}

/// Static metadata describing a component and how to construct it.
///
/// `factories` returns the component's `{Type}Factories` slice (the field-injection
/// default plus any `#[init]`/`factory =` contributions); deps live on each factory,
/// not here. There is exactly one `ComponentDescriptor` per type.
///
/// `Copy` so the registry can own a flat `Vec<ComponentDescriptor>` holding both
/// link-time-collected descriptors and ones synthesized at runtime for
/// manually-provided instances.
///
/// # Pre-1.0 descriptor migration
///
/// Handwritten literals must set [`generation_snapshot`](Self::generation_snapshot) to
/// `Some(GenerationSnapshot::of::<T>())` for a typed, retainable component or `None` for a
/// raw/manual descriptor. Prefer [`of`](Self::of) (with
/// [`with_identity`](Self::with_identity) for a custom identity) or
/// [`manual`](Self::manual); generated descriptors are updated automatically.
#[derive(Clone, Copy)]
pub struct ComponentDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub ty: TypeDescriptor,
    pub scope: ScopeKey,
    /// Optional root condition. Provider mappings inherit this component's eligibility.
    pub condition: Option<&'static ConditionDescriptor>,
    pub factories: fn() -> &'static [ComponentFactoryDescriptor],
    /// The component's `{Type}Hooks` slice (its `#[hook]` methods). Empty for a type
    /// that declares none — and for every manually-seeded instance.
    pub hooks: fn() -> &'static [HookDescriptor],
    /// Hidden typed generation adapter. Set for generated/typed descriptors
    /// ([`of`](Self::of) and macro-generated literals);
    /// `None` for raw [`manual`](Self::manual) descriptors, which are never
    /// retainable and expose no live cell.
    /// [`without_retention`](Self::without_retention) keeps it for live-cell access only.
    #[doc(hidden)]
    pub generation_snapshot: Option<GenerationSnapshot>,
}

/// The empty factory slice for a manually-provided instance: nothing to construct,
/// the value is seeded into the container directly.
fn no_factories() -> &'static [ComponentFactoryDescriptor] {
    &[]
}

impl ComponentDescriptor {
    pub const fn of<T: Component>() -> Self {
        Self {
            id: T::ID,
            name: T::NAME,
            ty: TypeDescriptor::of::<T>(T::NAME),
            scope: Singleton::KEY,
            condition: None,
            factories: no_factories,
            hooks: no_hooks,
            generation_snapshot: Some(GenerationSnapshot::of::<T>()),
        }
    }

    /// A descriptor for a manually-provided instance (no factory): the value is
    /// seeded into the container directly. Used for framework-seeded injectables
    /// (the peer, the shutdown handle) that carry a non-singleton or custom identity
    /// that [`of`](Self::of) does not express.
    pub const fn manual(
        id: &'static str,
        name: &'static str,
        ty: TypeDescriptor,
        scope: ScopeKey,
    ) -> Self {
        Self {
            id,
            name,
            ty,
            scope,
            condition: None,
            factories: no_factories,
            hooks: no_hooks,
            generation_snapshot: None,
        }
    }

    /// Whether a runtime transition can carry this component's instance into a later
    /// generation: a typed descriptor that has not opted out of retention.
    #[doc(hidden)]
    pub fn is_retainable(&self) -> bool {
        self.generation_snapshot
            .is_some_and(|adapter| adapter.retention().is_some())
    }

    /// Whether the runtime can read the live cell of this component's stored slot, so its
    /// live consumers can be rebound in place. True for every typed descriptor, retained
    /// or not; raw [`manual`](Self::manual) descriptors carry no typed access.
    #[doc(hidden)]
    pub fn exposes_live_cell(&self) -> bool {
        self.generation_snapshot.is_some()
    }

    /// The dependencies of the effective factory (empty for a manual instance, or if
    /// the factory choice is ambiguous — that is surfaced separately during validation).
    pub fn dependencies(&self) -> Vec<DependencyDescriptor> {
        match self.effective_factory().ok().flatten() {
            Some(factory) => (factory.dependencies)(),

            None => Vec::new(),
        }
    }
}

impl fmt::Debug for ComponentDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComponentDescriptor")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("ty", &self.ty)
            .field("scope", &self.scope.name())
            .field("condition", &self.condition)
            .field("dependencies", &self.dependencies())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
