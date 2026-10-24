use std::any::TypeId;
use std::collections::HashMap;

use upwell_core::TypeDescriptor;
use upwell_di::ComponentDescriptor;

/// The stable IDs of the components owning one generation's hooks, keyed by owner type.
///
/// A generation's hook manager is built from its resolved components' `#[hook]` methods,
/// so every hook a manager fires is owned by one of those components. Reports, rejections,
/// and traces name a hook's owner by this ID rather than its type descriptor name, so
/// they correlate with the component entries of the same report.
pub(crate) struct HookOwners {
    ids: HashMap<TypeId, &'static str>,
}

impl HookOwners {
    pub(crate) fn new(components: &[ComponentDescriptor]) -> Self {
        let ids = components
            .iter()
            .map(|descriptor| (descriptor.ty.type_id, descriptor.id))
            .collect();

        Self { ids }
    }

    /// The stable ID of the component owning a hook declared on `owner`. An owner outside
    /// the generation breaks the manager invariant, so it falls back to the type name.
    pub(crate) fn id(&self, owner: &TypeDescriptor) -> &'static str {
        let id = self.ids.get(&owner.type_id).copied();

        debug_assert!(
            id.is_some(),
            "a hook owner is missing from its generation's components"
        );

        id.unwrap_or(owner.name)
    }
}
