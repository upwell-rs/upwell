use std::sync::Arc;

use upwell_core::{Singleton, TypeDescriptor};
use upwell_di::{BoxedComponent, ComponentDescriptor, ConditionFactSnapshot, Injectable};
use upwell_hooks::{HOOK_MANAGER_ID, HOOK_MANAGER_NAME, HookManager};

use crate::runtime::AppConditionState;
use crate::{AppRegistry, PreparedScopeTopology, ScopeTopology};

/// The descriptor the app seeds for its hook manager.
pub(crate) fn hook_manager_descriptor() -> ComponentDescriptor {
    ComponentDescriptor::manual(
        HOOK_MANAGER_ID,
        HOOK_MANAGER_NAME,
        TypeDescriptor::of::<HookManager>(HOOK_MANAGER_NAME),
        &Singleton,
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
