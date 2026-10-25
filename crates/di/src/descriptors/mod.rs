//! Component and provider descriptors, and the link-time slices that collect them.

mod builder;
pub mod component;

pub use builder::{erase_provider, provider_live_cell};

pub use component::{
    BoxedComponent, Component, ComponentConstructionContext, ComponentDescriptor,
    ComponentFactories, ComponentFactory, ComponentFactoryDescriptor, ComponentFactoryRole,
    DeclaredScope, Dep, Dynamic, GenerationSnapshot, Injectable, Live, LiveCell, LiveRebind,
    LiveRef, Provided, ProviderDescriptor, ProviderOf, ProviderOrder, ProviderOrderDirection,
    Registration, ServiceComponent, Wired, Wiring,
};

pub use upwell_core::{
    Cardinality, ConditionDescriptor, DependencyDescriptor, DependencyObservation, DescriptorFor,
    ProviderMappingId, RegistryFor, ResolutionMode, Scope, ScopeId, ScopeKey, Singleton, Transient,
    TypeDescriptor, UpwellDescriptor,
};

/// Link-time registry of every discovered [`ComponentDescriptor`].
///
/// Proc macros register one element each via `#[linkme::distributed_slice(COMPONENTS)]`:
/// a `#[component]`/`#[service]` factory descriptor. The component registry reads the
/// assembled slice; each slice is homogeneous and assembled at link time with no
/// per-startup registration walk.
#[linkme::distributed_slice]
pub static COMPONENTS: [ComponentDescriptor];

/// Link-time registry of every discovered [`ProviderDescriptor`] (a component declaring
/// `provide = dyn Trait`). See [`COMPONENTS`].
#[linkme::distributed_slice]
pub static PROVIDERS: [ProviderDescriptor];
