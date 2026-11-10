//! Eager components: built on every opening of their scope, whatever the entry.

use super::*;
use crate::ScopedConstruction;

/// The layout of [`layout`] with the branch made eager.
fn eager_layout() -> Arc<crate::SlotLayout> {
    let mut branch = branch_descriptor();

    branch.construction = ScopedConstruction::Eager;

    Arc::new(plan(&[leaf_descriptor(), branch, other_descriptor()]))
}

fn ids<'a>(components: impl Iterator<Item = &'a ComponentDescriptor>) -> Vec<&'static str> {
    components.map(|component| component.id).collect()
}

#[test]
fn a_layout_lists_only_its_eager_components() {
    let layout = eager_layout();

    assert_eq!(ids(layout.eager_components()), ["branch"]);
    assert_eq!(ids(super::layout().eager_components()), Vec::<&str>::new());
}

#[tokio::test]
async fn an_entry_injecting_nothing_builds_the_eager_set() {
    let layout = eager_layout();
    let plan = entry_plan(&layout, &[]);

    let scope = open(&layout, Some(&plan)).await;

    assert!(
        !plan.is_empty(),
        "an eager component makes every plan build"
    );
    assert_eq!(ids(plan.components(&layout)), ["leaf", "branch"]);
    assert_eq!(
        builds(),
        [1, 1, 0],
        "the eager branch and the leaf it reads"
    );
    assert!(scope.get::<Branch>().is_some());
}

#[tokio::test]
async fn an_entry_adds_its_targets_to_the_eager_set() {
    let layout = eager_layout();
    let plan = entry_plan(&layout, &[<Arc<Other> as FromContainer>::dependency()]);

    open(&layout, Some(&plan)).await;

    assert_eq!(plan.len(), 3);
    assert_eq!(builds(), [1, 1, 1]);
}

#[tokio::test]
async fn a_scope_opened_on_demand_builds_its_eager_set() {
    let layout = eager_layout();
    let plan = crate::EntryPlan::on_demand(&layout);

    let scope = open(&layout, Some(&plan)).await;

    assert_eq!(ids(plan.components(&layout)), ["leaf", "branch"]);
    assert_eq!(builds(), [1, 1, 0]);
    assert!(
        scope.get::<Other>().is_none(),
        "other slots wait until resolved"
    );
}

#[test]
fn a_strict_entry_declares_the_eager_set() {
    let layout = eager_layout();

    let plan = entry_plan_under(&layout, &[], UnplannedConstruction::Deny);

    assert_eq!(
        plan.declares().map(|declares| &**declares),
        Some(&[true, true, false][..]),
        "the eager branch and the leaf it reads, not the other slot"
    );
}

#[test]
fn without_eager_components_an_on_demand_plan_builds_nothing() {
    let layout = super::layout();

    let plan = crate::EntryPlan::on_demand(&layout);

    assert!(plan.is_empty());
}
