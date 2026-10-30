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

use upwell_core::{
    Cardinality, DependencyDescriptor, DependencyObservation, ResolutionMode, TypeDescriptor,
};

mod factory;
mod from_container;
mod handoff;
mod hot_update;
mod one_shot;
mod replacement;

pub use factory::{
    Factory, FactoryOutput, ReadyFactory, dispatch_factory, dispatch_ready_factory,
    factory_dependencies,
};
pub use from_container::FromContainer;
pub use hot_update::{
    HotUpdate, HotUpdateCommit, HotUpdateFactory, HotUpdateOutput, dispatch_hot_update,
    hot_update_dependencies,
};
pub use one_shot::{ComponentChange, OneShot, dispatch_transition};
pub(crate) use one_shot::{ErasedOneShot, take_selection};
pub use replacement::{
    Previous, ReplacementFactory, dispatch_replacement, replacement_dependencies,
};

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
