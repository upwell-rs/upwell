//! Runtime live rebinding depends on the target as well as the consumer. A retained
//! consumer is rebound in place whenever its live target exposes a typed live cell, even
//! when the target itself is never retained. A target without typed access reconstructs
//! the consumer (which is displaced) instead of failing during preparation.

use upwell_di::{BindingTransition, Dep};

use super::*;
use crate::transition::ResolvedTransitionPlan;

/// A singleton reached only through a raw descriptor without typed access, so the
/// runtime cannot read its live cell.
pub(super) struct UntypedTarget;

test_component!(UntypedTarget, "untyped-target");

fn untyped_target() -> ComponentDescriptor {
    async fn construct() -> UntypedTarget {
        UntypedTarget
    }

    ComponentDescriptor {
        generation_snapshot: None,
        ..ComponentDescriptor::of::<UntypedTarget>()
            .with_factories(upwell_di::factories!["untyped-target" => construct])
    }
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
    UnretainedWatcher,
    "unretained-watcher",
    unretained_watcher,
    InvalidatedRoot
);
live_watcher!(
    UntypedWatcher,
    "untyped-watcher",
    untyped_watcher,
    UntypedTarget
);

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
fn a_never_retained_live_target_still_rebinds_its_consumer_in_place() {
    let target = invalidated_root();

    assert!(!target.is_retainable() && target.exposes_live_cell());

    let plan = reconstruct_target(target, unretained_watcher());

    assert!(binding_of(&plan, "unretained-watcher").in_place);
    assert_eq!(
        strategy_of(&plan, "unretained-watcher"),
        Some(ComponentTransitionStrategy::Retain)
    );
    assert!(!plan.displaced().contains(&"unretained-watcher"));
}

#[test]
fn a_live_target_without_typed_access_reconstructs_its_consumer() {
    let plan = reconstruct_target(untyped_target(), untyped_watcher());

    assert!(
        !binding_of(&plan, "untyped-watcher").in_place,
        "the target's live cell is unreachable, so the binding cannot rebind in place"
    );
    assert_eq!(
        strategy_of(&plan, "untyped-watcher"),
        Some(ComponentTransitionStrategy::Reconstruct)
    );
    assert!(
        plan.displaced().contains(&"untyped-watcher"),
        "the reconstructed watcher's active instance is displaced"
    );
}
