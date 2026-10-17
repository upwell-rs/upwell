//! Descriptors for planning and validation tests over marker types: components whose
//! factories are never driven and provider mappings that are never erased.

use std::future::Future;
use std::pin::Pin;

use upwell_core::{
    Cardinality, DependencyDescriptor, DependencyObservation, ResolutionMode, Singleton,
    TypeDescriptor,
};

use crate::descriptors::{
    BoxedComponent, ComponentConstructionContext, ComponentDescriptor, ComponentFactoryDescriptor,
    ProviderDescriptor,
};

/// Declares the one never-driven recipe `$id` declaring `$dependencies` as the
/// `fn() -> &'static [..]` a descriptor stores; `default` marks it the fallback recipe.
macro_rules! unbuilt_factories {
    ($id:literal, $dependencies:expr) => {
        $crate::test_support::unbuilt_factories!($id, $dependencies, false)
    };
    ($id:literal, $dependencies:expr, $default:literal) => {
        || {
            const {
                &[$crate::descriptors::ComponentFactoryDescriptor {
                    id: $id,
                    construct: $crate::test_support::unbuilt,
                    dependencies: $dependencies,
                    default: $default,
                }]
            }
        }
    };
}

pub(crate) use unbuilt_factories;

/// The factory callback of a recipe that planning and validation never drive.
pub(crate) fn unbuilt(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + '_>> {
    panic!("a planning or validation test constructed a component")
}

/// The erase callback of a provider mapping that planning and validation never erase.
pub(crate) fn unerased(_: &BoxedComponent) -> BoxedComponent {
    panic!("a planning or validation test erased a provider")
}

pub(crate) fn no_dependencies() -> Vec<DependencyDescriptor> {
    Vec::new()
}

/// A singleton component of type `ty` named and identified `id`, built by `factories`,
/// with no condition and no hooks, and never retained across generations.
pub(crate) const fn planned(
    id: &'static str,
    ty: TypeDescriptor,
    factories: fn() -> &'static [ComponentFactoryDescriptor],
) -> ComponentDescriptor {
    ComponentDescriptor {
        id,
        name: id,
        ty,
        scope: &Singleton,
        condition: None,
        factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    }
}

/// The priority-0, unordered mapping of `concrete` to `trait_ty` under `qualifier`,
/// primary when `primary`; adjust it with the [`ProviderDescriptor`] setters.
pub(crate) const fn planned_provider(
    trait_ty: TypeDescriptor,
    concrete: TypeDescriptor,
    qualifier: &'static str,
    primary: bool,
) -> ProviderDescriptor {
    ProviderDescriptor {
        trait_ty,
        concrete_ty: concrete,
        qualifier,
        primary,
        priority: 0,
        ordering: &[],
        erase: unerased,
    }
}

/// [`planned`] for marker type `T`, whose type descriptor is named `id`.
pub(crate) const fn component<T: 'static>(
    id: &'static str,
    factories: fn() -> &'static [ComponentFactoryDescriptor],
) -> ComponentDescriptor {
    planned(id, TypeDescriptor::of::<T>(id), factories)
}

/// [`planned_provider`] for marker type `T`, whose type descriptor is named by its Rust
/// type name.
pub(crate) fn provider<T: 'static>(
    trait_ty: TypeDescriptor,
    qualifier: &'static str,
    primary: bool,
) -> ProviderDescriptor {
    planned_provider(
        trait_ty,
        TypeDescriptor::of::<T>(std::any::type_name::<T>()),
        qualifier,
        primary,
    )
}

/// A required, unqualified, non-config snapshot dependency on `ty` named `name`.
pub(crate) const fn dependency(
    name: &'static str,
    ty: TypeDescriptor,
    cardinality: Cardinality,
    resolution: ResolutionMode,
) -> DependencyDescriptor {
    DependencyDescriptor {
        name,
        ty,
        cardinality,
        optional: false,
        dynamic: false,
        qualifier: None,
        config: false,
        resolution,
        observation: DependencyObservation::Snapshot,
    }
}
