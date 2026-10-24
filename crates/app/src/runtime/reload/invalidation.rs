use std::any::TypeId;
use std::collections::BTreeSet;

use upwell_config::ConfigReload;
use upwell_di::EffectiveTarget;
use upwell_hooks::HOOK_MANAGER_ID;

use crate::transition::CandidateGraph;

/// Computes the deterministic set of candidate component roots a non-empty staged
/// reload must force to reconstruct.
///
/// A root is invalidated when any of the following holds:
///
/// 1. One of its config dependency targets matches a changed binding input — an exact
///    `(TypeId, path)` pair derived from the changed staged entries — by type identity
///    and, when the dependency is qualified, by exactly that path. An unqualified
///    dependency matches by type identity alone.
/// 2. It owns a `ConfigReload` hook — conservatively invalidated on any changed reload,
///    regardless of which binding changed.
/// 3. It depends on the `HookManager` singleton, which is generation-local and therefore
///    always fresh in the candidate root.
///
/// The result is a `BTreeSet`, so overlapping criteria deduplicate and iteration order
/// is deterministic.
pub(super) fn invalidated_component_roots(
    candidate: &CandidateGraph,
    changed: &[(TypeId, &str)],
) -> BTreeSet<&'static str> {
    let mut invalidated = BTreeSet::new();

    for node in candidate.graph().nodes() {
        let binding_changed = node.dependencies.iter().any(|dependency| {
            dependency
                .targets
                .iter()
                .any(|target| config_target_matches(target, changed))
        });

        let consumes_hook_manager = node.dependencies.iter().any(|dependency| {
            dependency
                .targets
                .iter()
                .any(|target| target.component_id() == Some(HOOK_MANAGER_ID))
        });

        if binding_changed || consumes_hook_manager {
            invalidated.insert(node.id);
        }
    }

    for component in candidate.components() {
        let owns_config_reload_hook = (component.hooks)()
            .iter()
            .any(|hook| (hook.kind_ty)() == TypeId::of::<ConfigReload>());

        if owns_config_reload_hook {
            invalidated.insert(component.id);
        }
    }

    invalidated
}

/// Whether one effective dependency target matches any changed binding input.
///
/// Only config targets can match, and only on exact type identity: the target carries
/// the dependency type's `TypeId` and each changed input carries the staged binding's
/// `TypeId`. A qualified target matches only its exact binding path; an unqualified
/// target matches by type identity alone.
fn config_target_matches(target: &EffectiveTarget, changed: &[(TypeId, &str)]) -> bool {
    let EffectiveTarget::Config {
        config_type_id,
        binding_path,
        ..
    } = *target
    else {
        return false;
    };

    changed.iter().any(|(changed_type_id, changed_path)| {
        config_type_id == *changed_type_id
            && binding_path.is_none_or(|binding_path| binding_path == *changed_path)
    })
}
