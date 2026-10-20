use upwell_core::{
    Cardinality, DependencyDescriptor, DependencyObservation, ResolutionMode, TypeDescriptor,
};
use upwell_di::{BoxedComponent, ProviderDescriptor};

/// The erase callback of a provider mapping that planning tests register but never erase.
pub(crate) fn unerased(_: &BoxedComponent) -> BoxedComponent {
    panic!("a planning test erased a provider")
}

/// The priority-0, unordered, never-erased mapping of `concrete` to `trait_ty` under
/// `qualifier`, primary when `primary`; adjust it with the [`ProviderDescriptor`] setters.
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
        live: |_| None,
    }
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
