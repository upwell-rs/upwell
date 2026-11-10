//! Scopes opened for an entry under `UnplannedConstruction::Deny`, which build on demand only
//! what the entry declares.

use super::*;
use crate::Error;

/// Opens `layout` for an entry injecting `targets`, denying undeclared on-demand builds.
async fn open_denying(
    layout: &Arc<crate::SlotLayout>,
    targets: &[DependencyDescriptor],
) -> Arc<ScopeContainer> {
    let plan = entry_plan_under(layout, targets, UnplannedConstruction::Deny);

    open(layout, Some(&plan)).await
}

#[tokio::test]
async fn an_undeclared_slot_is_not_built_on_demand() {
    let layout = layout();
    let scope = open_denying(&layout, &[<Arc<Branch> as FromContainer>::dependency()]).await;

    let other = scope.resolve::<Arc<Other>>().await;

    assert!(
        matches!(
            other,
            Err(Error::UndeclaredConstruction {
                component: "other",
                ..
            })
        ),
        "the entry does not declare the other component"
    );
    assert_eq!(builds(), [1, 1, 0], "nothing undeclared was built");
}

#[tokio::test]
async fn a_lazy_target_declares_its_slots_for_on_demand_builds() {
    let layout = layout();
    let scope = open_denying(
        &layout,
        &[<crate::Lazy<Arc<Branch>> as FromContainer>::dependency()],
    )
    .await;

    let branch = scope
        .resolve::<Arc<Branch>>()
        .await
        .expect("the lazily declared branch builds on demand");
    let other = scope.resolve::<Arc<Other>>().await;

    assert!(branch.is_some());
    assert_eq!(builds(), [1, 1, 0], "the branch and the leaf it reads");
    assert!(matches!(other, Err(Error::UndeclaredConstruction { .. })));
}

#[tokio::test]
async fn a_scope_opened_without_an_entry_builds_on_demand_under_either_policy() {
    let layout = layout();
    let plan = crate::EntryPlan::on_demand(&layout);
    let on_demand = open(&layout, Some(&plan)).await;
    let everything = open(&layout, None).await;

    let other = on_demand
        .resolve::<Arc<Other>>()
        .await
        .expect("a scope building on demand has no declared entry");

    assert!(other.is_some());
    assert!(everything.get::<Other>().is_some());
}
