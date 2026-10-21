//! Runtime live rebinding depends on the target as well as the consumer: a retained
//! consumer is rebound in place only when its live target exposes a live cell, and is
//! otherwise reconstructed (and displaced) rather than failing during preparation.

use upwell_di::{BindingTransition, Dep};

use super::*;
use crate::transition::ResolvedTransitionPlan;

/// A singleton whose descriptor keeps its generation-snapshot adapter, so the runtime
/// can reach its live cell.
pub(super) struct AdaptedTarget;

test_component!(AdaptedTarget, "adapted-target");

fn adapted_target() -> ComponentDescriptor {
    async fn construct() -> AdaptedTarget {
        AdaptedTarget
    }

    ComponentDescriptor::of::<AdaptedTarget>()
        .with_factories(upwell_di::factories!["adapted-target" => construct])
}

/// Declares a retainable unit consumer `$ty` holding a live `Dep<$target>`.
macro_rules! live_watcher {
    ($ty:ident, $id:literal, $descriptor:ident, $target:ty) => {
        pub(super) struct $ty;

        test_component!($ty, $id);

        fn $descriptor() -> ComponentDescriptor {
            async fn construct(_: Dep<$target>) -> $ty {
                $ty
            }

            ComponentDescriptor::of::<$ty>()
                .with_factories(upwell_di::factories![$id => construct])
        }
    };
}

live_watcher!(
    AdaptedWatcher,
    "adapted-watcher",
    adapted_watcher,
    AdaptedTarget
);
live_watcher!(BareWatcher, "bare-watcher", bare_watcher, InvalidatedRoot);

/// Resolves a runtime transition that reconstructs `target` under its live `watcher`.
fn reconstruct_target(
    target: ComponentDescriptor,
    watcher: ComponentDescriptor,
) -> ResolvedTransitionPlan {
    let target_id = target.id;
    let (active, candidate) = unchanged_graphs([target, watcher]);

    candidate
        .resolve_runtime_transition(&active, &BTreeSet::from([target_id]))
        .expect("the live closure resolves without restart")
}

fn binding_of(plan: &ResolvedTransitionPlan, consumer: &str) -> BindingTransition {
    *plan
        .structural()
        .bindings
        .iter()
        .find(|binding| binding.dependency.consumer == consumer)
        .expect("the watcher's live binding is planned")
}

fn strategy_of(
    plan: &ResolvedTransitionPlan,
    component: &str,
) -> Option<ComponentTransitionStrategy> {
    plan.decisions()
        .iter()
        .find(|decision| decision.component == component)
        .map(|decision| decision.strategy)
}

#[test]
fn a_live_target_with_a_snapshot_adapter_rebinds_its_consumer_in_place() {
    let plan = reconstruct_target(adapted_target(), adapted_watcher());

    assert!(binding_of(&plan, "adapted-watcher").in_place);
    assert_eq!(
        strategy_of(&plan, "adapted-watcher"),
        Some(ComponentTransitionStrategy::Retain)
    );
    assert!(!plan.displaced().contains(&"adapted-watcher"));
}

#[test]
fn a_live_target_without_a_snapshot_adapter_reconstructs_its_consumer() {
    let plan = reconstruct_target(invalidated_root(), bare_watcher());

    assert!(
        !binding_of(&plan, "bare-watcher").in_place,
        "the target's live cell is unreachable, so the binding cannot rebind in place"
    );
    assert_eq!(
        strategy_of(&plan, "bare-watcher"),
        Some(ComponentTransitionStrategy::Reconstruct)
    );
    assert!(
        plan.displaced().contains(&"bare-watcher"),
        "the reconstructed watcher's active instance is displaced"
    );
}
