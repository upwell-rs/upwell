use std::any::TypeId;
use std::cmp::Reverse;
use std::collections::HashMap;
use std::sync::Arc;

use upwell_core::{Scope, Singleton, TypeDescriptor};
use upwell_di::{
    BoxedComponent, ComponentDescriptor, ConditionFactSnapshot, Injectable, ProviderSelectionModel,
    SlotLayout,
};
use upwell_hooks::{HOOK_MANAGER_ID, HOOK_MANAGER_NAME, HookManager};

use crate::runtime::AppConditionState;
use crate::scope::{ScopeLayouts, SeedDestination};
use crate::{AppRegistry, PreparedScopeTopology, ScopeTopology};

/// The descriptor the app seeds for its hook manager.
pub(crate) fn hook_manager_descriptor() -> ComponentDescriptor {
    ComponentDescriptor::manual(
        HOOK_MANAGER_ID,
        HOOK_MANAGER_NAME,
        TypeDescriptor::of::<HookManager>(HOOK_MANAGER_NAME),
        Singleton::KEY,
    )
}

/// The seeded instance of `hooks`, as a root build expects it.
pub(crate) fn seed_hook_manager(hooks: &HookManager) -> BoxedComponent {
    BoxedComponent {
        ty: TypeDescriptor::of::<HookManager>(HOOK_MANAGER_NAME),
        value: Box::new(Injectable::into_stored(hooks.clone())),
    }
}

/// The condition state of an empty registry with no condition facts.
pub(crate) fn empty_condition() -> Arc<AppConditionState> {
    let registry = AppRegistry::default();
    let evaluation = registry
        .evaluate_conditions(
            [],
            &ConditionFactSnapshot::new([]).expect("empty snapshot validates"),
        )
        .expect("empty evaluation succeeds");

    Arc::new(AppConditionState::new(Arc::new(registry), evaluation))
}

/// The prepared topology with no protocol scopes.
pub(crate) fn empty_topology() -> PreparedScopeTopology {
    ScopeTopology::empty()
        .prepare()
        .expect("empty topology validates")
}

/// The slot layouts of `topology` when every scope holds only the seeds `destinations`
/// declares for it, as a generation without components plans them.
pub(crate) fn seed_layouts(
    topology: &PreparedScopeTopology,
    destinations: &HashMap<TypeId, SeedDestination>,
    selection: &ProviderSelectionModel,
) -> ScopeLayouts {
    let mut boundaries: Vec<_> = topology.boundaries().iter().collect();
    let mut layouts = HashMap::new();

    boundaries.sort_by_key(|boundary| Reverse(boundary.rank()));

    for boundary in boundaries {
        let seeded: Vec<ComponentDescriptor> = destinations
            .iter()
            .filter(|(_, destination)| destination.scope == boundary.id())
            .map(|(type_id, destination)| {
                let ty = TypeDescriptor {
                    name: destination.type_name,
                    type_name: || "seed",
                    type_id: *type_id,
                    source: None,
                };

                ComponentDescriptor::manual(
                    destination.type_name,
                    destination.type_name,
                    ty,
                    boundary.scope(),
                )
            })
            .collect();
        let layout = SlotLayout::plan(
            boundary.scope(),
            &seeded,
            &[],
            &layouts,
            selection,
            |consumer, dependency| topology.is_reachable(&consumer, &dependency),
        )
        .expect("a seed-only layout plans");

        layouts.insert(boundary.id(), Arc::new(layout));
    }

    layouts
}
