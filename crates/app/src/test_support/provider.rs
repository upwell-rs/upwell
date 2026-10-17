use upwell_core::TypeDescriptor;
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
    }
}
