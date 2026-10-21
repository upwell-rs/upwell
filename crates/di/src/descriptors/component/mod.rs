use std::ops::Deref;
use std::{
    any::{Any, TypeId},
    collections::HashMap,
    fmt,
    sync::Arc,
};

use upwell_core::{
    ConditionDescriptor, DependencyDescriptor, ProviderMappingId, Scope, Singleton, TypeDescriptor,
};
use upwell_hooks::{HookDescriptor, no_hooks};

mod context;
mod factory;
mod live;

pub use context::ComponentConstructionContext;
pub use factory::{
    ComponentFactories, ComponentFactory, ComponentFactoryDescriptor, ComponentFactoryRole,
    Registration,
};
pub use live::{Dep, Live, LiveCell, LiveRebind, LiveRef};

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

/// Compile-time dependency marker: `Wiring: Provide<T>` holds when some component
/// in scope provides `T`. Under the `di-check` feature, the macros emit
/// `impl Provide<Self> for Wiring` for every component and a bound per concrete
/// dependency, so a missing provider is a `cargo check` error.
#[diagnostic::on_unimplemented(
    message = "no component provides `{T}`",
    note = "add a component for `{T}`, or mark the dependency `Dynamic<{T}>` if it is provided at runtime"
)]
pub trait Provide<T: ?Sized> {}

/// A concrete component that provides the trait object `T`.
///
/// Generated by `provide = dyn Trait`; implement it by hand to declare a provider
/// without the macros (see [`ProviderDescriptor::of`]). Also bounds explicitly
/// trait-restricted provider ordering.
pub trait ProviderOf<T: ?Sized> {
    /// Unsizes the shared concrete instance into the provided trait object.
    fn upcast(this: Arc<Self>) -> Arc<T>;
}

/// The crate-wide anchor that each component's `Provide` impl attaches to. A
/// dependency assertion is the bound `Wiring: Provide<Dep>`.
pub struct Wiring;

/// Marker that all of a component's dependencies are provided. Under `di-check`
/// the macros emit `impl Wired for T where Wiring: Provide<Dep>, ..` carrying
/// *every* single dependency (concrete and trait-object) as a lazy bound.
pub trait Wired {}

/// Direction of a relative provider-ordering constraint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderOrderDirection {
    /// The source provider precedes the target provider.
    Before,
    /// The source provider follows the target provider.
    After,
}

/// One target in a provider's static relative-ordering metadata.
#[derive(Clone, Copy, Debug)]
pub struct ProviderOrder {
    /// Concrete component type targeted by the constraint.
    pub target: TypeDescriptor,
    /// Traits restricted by `as`; empty means every trait provided by both the
    /// source and target components.
    pub traits: &'static [TypeDescriptor],
    /// Whether the source belongs before or after the target.
    pub direction: ProviderOrderDirection,
}

/// Declares that a component **provides** a trait, so it can be injected as a
/// single `Arc<dyn Trait>` (the primary or sole provider), or collected into a
/// `Vec<Arc<dyn Trait>>` / `HashMap<String, Arc<dyn Trait>>` of all providers.
///
/// Emitted by `#[component(provide = ..)]` / `#[service(provide = ..)]`, one per
/// `(component, trait)` pair. `#[primary]` on the component sets `primary` on
/// every trait it provides; the dependency side never names a primary — it asks
/// for `Arc<dyn Trait>` and the container resolves the right one.
#[derive(Clone, Copy)]
pub struct ProviderDescriptor {
    /// The trait-object type provided, keyed by `TypeId::of::<dyn Trait>()`.
    pub trait_ty: TypeDescriptor,
    /// The providing component's concrete type.
    pub concrete_ty: TypeDescriptor,
    /// Qualifier key for `HashMap<String, Arc<dyn Trait>>` injection. Always
    /// present — inferred from the component's id unless set with `qualifier`.
    pub qualifier: &'static str,
    /// Whether this is the primary provider — chosen for a single `Arc<dyn Trait>`
    /// dependency when several providers of the trait exist.
    pub primary: bool,
    /// Collection-order priority. Lower values precede higher values among
    /// providers whose relative `before`/`after` constraints are satisfied.
    pub priority: i64,
    /// Relative ordering constraints declared on the component.
    pub ordering: &'static [ProviderOrder],
    /// Re-erases the built concrete instance (`Arc<Concrete>`) as `Arc<dyn Trait>`
    /// for storage under the trait's key. Generated by the macro, which alone
    /// knows that `Concrete: Trait`.
    pub erase: fn(&BoxedComponent) -> BoxedComponent,
    /// Returns the live cell an [`erase`](Self::erase)d trait slot holds, so a runtime
    /// transition can republish the `Dep<dyn Trait>` handles consumers took from it.
    #[doc(hidden)]
    pub live: fn(&BoxedComponent) -> Option<LiveCell>,
}

impl fmt::Debug for ProviderDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderDescriptor")
            .field("trait_ty", &self.trait_ty)
            .field("concrete_ty", &self.concrete_ty)
            .field("qualifier", &self.qualifier)
            .field("primary", &self.primary)
            .field("priority", &self.priority)
            .field("ordering", &self.ordering)
            .finish_non_exhaustive()
    }
}

impl ProviderDescriptor {
    /// Returns the stable mapping identity relative to its validated concrete component.
    pub fn mapping_id(&self, component: &ComponentDescriptor) -> ProviderMappingId {
        ProviderMappingId {
            component: component.id,
            trait_type: self.trait_ty.type_name,
            qualifier: self.qualifier,
        }
    }
}

/// A type-erased instantiated component.
///
/// `value` stores `Arc<T>` inside a `Box<dyn Any + Send + Sync>`, which
/// allows recovery via `value.downcast_ref::<Arc<T>>()`.
pub struct BoxedComponent {
    pub ty: TypeDescriptor,
    pub value: Box<dyn Any + Send + Sync>,
}

impl AsRef<Box<dyn Any + Send + Sync>> for BoxedComponent {
    fn as_ref(&self) -> &Box<dyn Any + Send + Sync> {
        &self.value
    }
}

impl Deref for BoxedComponent {
    type Target = Box<dyn Any + Send + Sync>;

    fn deref(&self) -> &Box<dyn Any + Send + Sync> {
        &self.value
    }
}

impl fmt::Debug for BoxedComponent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoxedComponent")
            .field("ty", &self.ty)
            .finish_non_exhaustive()
    }
}

/// One trait provider's constructed value, plus the metadata that selects it.
pub(crate) struct ProviderInstance {
    pub(crate) descriptor: ProviderDescriptor,
    pub(crate) ordinal: usize,
    pub(crate) value: BoxedComponent,
}

/// The instances and trait-providers held by one scope.
///
/// Shared by both the under-construction [`ComponentConstructionContext`] and the
/// frozen [`ScopeContainer`](crate::container::ScopeContainer); each owns one
/// `ScopeStore` and layers parent scopes on top for resolution. All lookups here
/// are **scope-local** — walking the parent chain is the caller's job. Config values
/// are *not* held here: config is an external resolver, not part of the container.
#[derive(Default)]
pub(crate) struct ScopeStore {
    pub(crate) components: HashMap<TypeId, BoxedComponent>,
    pub(crate) providers: HashMap<TypeId, Vec<ProviderInstance>>,
}

/// Recovers handle `H` from a boxed slot: downcasts to its stored representation
/// (`H::Stored`) and derives the handle. This is the single seam through which
/// every resolution converts storage back into a handle — a snapshot for `Arc<T>`,
/// a shared clone for `Dep<T>`, a plain clone for a by-value handle. `None` if the
/// box holds a different stored type.
///
/// Public so external resolvers (e.g. the config store) can recover a handle from a
/// seed they hold without re-implementing the downcast.
pub fn from_boxed<H: Injectable>(boxed: &BoxedComponent) -> Option<H> {
    boxed.value.downcast_ref::<H::Stored>().map(H::from_stored)
}

/// Hidden typed generation adapter: the optional [`Retention`] that carries an instance
/// into a later generation, plus the live-cell accessor that reads a stored slot's
/// [`Live<T>`] cell.
///
/// Live-cell access only needs the component's type, so it stays available when a
/// descriptor opts out of retention: live consumers of a never-retained component can
/// still be rebound in place.
#[derive(Clone, Copy)]
#[doc(hidden)]
pub struct GenerationSnapshot {
    retention: Option<Retention>,
    live: fn(&BoxedComponent) -> Option<LiveCell>,
}

/// The typed snapshot adapter that carries an instance into a later generation, and the
/// validator for the payload it produces.
#[derive(Clone, Copy)]
pub(crate) struct Retention {
    snapshot: fn(&BoxedComponent) -> crate::Result<BoxedComponent>,
    validate: fn(&BoxedComponent) -> bool,
}

impl Retention {
    pub(crate) fn snapshot(self, active: &BoxedComponent) -> crate::Result<BoxedComponent> {
        (self.snapshot)(active)
    }

    pub(crate) fn validates(self, candidate: &BoxedComponent) -> bool {
        (self.validate)(candidate)
    }
}

impl GenerationSnapshot {
    /// The adapter that carries the instance into a later generation, unless the
    /// descriptor opted out of retention.
    pub(crate) fn retention(self) -> Option<Retention> {
        self.retention
    }

    pub(crate) fn live_cell(self, stored: &BoxedComponent) -> Option<LiveCell> {
        (self.live)(stored)
    }

    /// This adapter without retention, keeping its live-cell access.
    pub(crate) const fn without_retention(self) -> Self {
        Self {
            retention: None,
            ..self
        }
    }

    #[cfg(test)]
    pub(crate) const fn for_test(
        snapshot: fn(&BoxedComponent) -> crate::Result<BoxedComponent>,
        validate: fn(&BoxedComponent) -> bool,
    ) -> Self {
        Self {
            retention: Some(Retention { snapshot, validate }),
            live: |_| None,
        }
    }
}

/// The generic generation-snapshot adapter for a typed component: recovers the
/// handle from the active stored slot and boxes a *new* stored representation.
/// For an `Arc<T>` handle this shares the same `Arc` instance but creates an
/// independent [`Live<T>`] cell. By-value handles work only when their `Injectable`
/// implementation explicitly opts in (the component macro's `retainable` flag).
pub(crate) fn generation_snapshot_adapter<T: Component>(
    active: &BoxedComponent,
) -> crate::Result<BoxedComponent> {
    let stored = active
        .value
        .downcast_ref::<<T::Handle as Injectable>::Stored>()
        .ok_or(crate::Error::SnapshotStorageMismatch { component: T::ID })?;

    let candidate_stored = <T::Handle as Injectable>::snapshot_stored(stored)
        .ok_or(crate::Error::SnapshotHandleUnsupported { component: T::ID })?;

    Ok(BoxedComponent {
        ty: active.ty,
        value: Box::new(candidate_stored),
    })
}

/// The live cell a typed component's stored slot holds, when its handle stores one.
fn generation_live_cell<T: Component>(stored: &BoxedComponent) -> Option<LiveCell> {
    stored
        .value
        .downcast_ref::<<T::Handle as Injectable>::Stored>()
        .and_then(<T::Handle as Injectable>::live_cell)
}

fn generation_snapshot_validates<T: Component>(candidate: &BoxedComponent) -> bool {
    candidate
        .value
        .downcast_ref::<<T::Handle as Injectable>::Stored>()
        .is_some()
}

impl ScopeStore {
    /// Single concrete-or-primary-provider lookup, scope-local. `None` if absent or
    /// ambiguous.
    pub(crate) fn resolve_local<H: Injectable>(&self) -> Option<H> {
        let type_id = TypeId::of::<H::Target>();

        if let Some(component) = self.components.get(&type_id) {
            return from_boxed::<H>(component);
        }

        let chosen =
            crate::registry::selection::select_single_by(self.providers.get(&type_id)?, |entry| {
                entry.descriptor.primary
            })?;

        from_boxed::<H>(&chosen.value)
    }

    /// Qualifier-selected single provider, scope-local.
    pub(crate) fn resolve_qualified_local<H: Injectable>(&self, qualifier: &str) -> Option<H> {
        let type_id = TypeId::of::<H::Target>();
        let entry = self
            .providers
            .get(&type_id)?
            .iter()
            .find(|entry| entry.descriptor.qualifier == qualifier)?;

        from_boxed::<H>(&entry.value)
    }

    /// Resolves one exact provider descriptor from this scope.
    pub(crate) fn resolve_provider_local<H: Injectable>(
        &self,
        provider: &ProviderDescriptor,
    ) -> Option<H> {
        let entry = self
            .providers
            .get(&provider.trait_ty.type_id)?
            .iter()
            .find(|entry| {
                entry.descriptor.concrete_ty.type_id == provider.concrete_ty.type_id
                    && entry.descriptor.qualifier == provider.qualifier
            })?;

        from_boxed::<H>(&entry.value)
    }

    pub(crate) fn insert(&mut self, component: BoxedComponent) {
        let type_id = component.ty.type_id;
        self.components.insert(type_id, component);
    }

    /// Erases an already-constructed concrete component as `Arc<dyn Trait>` and
    /// records it under the trait's key, so trait-object dependencies resolve.
    pub(crate) fn register_provider(&mut self, provider: &ProviderDescriptor, ordinal: usize) {
        let concrete_id = provider.concrete_ty.type_id;

        let Some(concrete) = self.components.get(&concrete_id) else {
            return;
        };

        let erased = (provider.erase)(concrete);

        self.providers
            .entry(provider.trait_ty.type_id)
            .or_default()
            .push(ProviderInstance {
                descriptor: *provider,
                ordinal,
                value: erased,
            });

        self.providers
            .get_mut(&provider.trait_ty.type_id)
            .expect("provider entry was inserted")
            .sort_by_key(|provider| provider.ordinal);
    }

    pub(crate) fn contains(&self, type_id: TypeId) -> bool {
        self.components.contains_key(&type_id)
    }
}

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
/// [`ComponentDescriptor::typed_snapshot::<T>()`] for a typed, retainable component or
/// `None` for a raw/manual descriptor. Prefer [`of`](Self::of), [`manual`](Self::manual),
/// or [`manual_of`](Self::manual_of); generated descriptors are updated automatically.
#[derive(Clone, Copy)]
pub struct ComponentDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub ty: TypeDescriptor,
    pub scope: &'static dyn Scope,
    /// Optional root condition. Provider mappings inherit this component's eligibility.
    pub condition: Option<&'static ConditionDescriptor>,
    pub factories: fn() -> &'static [ComponentFactoryDescriptor],
    /// The component's `{Type}Hooks` slice (its `#[hook]` methods). Empty for a type
    /// that declares none — and for every manually-seeded instance.
    pub hooks: fn() -> &'static [HookDescriptor],
    /// Hidden typed generation adapter. Set for generated/typed descriptors
    /// ([`of`](Self::of), [`manual_of`](Self::manual_of), macro-generated literals);
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
            scope: &Singleton,
            condition: None,
            factories: no_factories,
            hooks: no_hooks,
            generation_snapshot: Self::typed_snapshot::<T>(),
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
        scope: &'static dyn Scope,
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

    /// A descriptor for a framework-seeded typed instance (no factory) that carries
    /// the typed generation-snapshot adapter, so the instance can be retained across
    /// generations. Raw [`manual`](Self::manual) descriptors stay non-retainable.
    #[doc(hidden)]
    pub const fn manual_of<T: Component>(
        id: &'static str,
        name: &'static str,
        scope: &'static dyn Scope,
    ) -> Self {
        Self {
            id,
            name,
            ty: TypeDescriptor::of::<T>(name),
            scope,
            condition: None,
            factories: no_factories,
            hooks: no_hooks,
            generation_snapshot: Self::typed_snapshot::<T>(),
        }
    }

    /// The typed generation-snapshot adapter for `T`, for descriptor literals that
    /// cannot go through [`of`](Self::of) — macro-generated descriptors and
    /// handwritten literals with custom identity or factory slices.
    #[doc(hidden)]
    pub const fn typed_snapshot<T: Component>() -> Option<GenerationSnapshot> {
        Some(GenerationSnapshot {
            retention: Some(Retention {
                snapshot: generation_snapshot_adapter::<T>,
                validate: generation_snapshot_validates::<T>,
            }),
            live: generation_live_cell::<T>,
        })
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
