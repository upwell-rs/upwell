//! Whether two layouts plan entries alike, and the types an entry plan reads.

use super::*;
use crate::layout::UnplannedConstruction;

fn plan(
    layout: &SlotLayout,
    selection: &ProviderSelectionModel,
    targets: &[DependencyDescriptor],
) -> EntryPlan {
    layout.plan_entry(targets, selection, reach, UnplannedConstruction::Deny)
}

#[test]
fn layouts_planned_from_the_same_components_plan_entries_alike() {
    let fixture = Fixture::new();
    let selection = fixture.selection(&[]);
    let order = [fixture.handler, fixture.consumer];

    let first = fixture
        .inner(&selection, &order)
        .expect("the inner scope plans");
    let second = fixture
        .inner(&selection, &order)
        .expect("the inner scope plans");
    let fewer = fixture
        .inner(&selection, &[fixture.handler])
        .expect("the inner scope plans without its consumer");

    assert!(first.plans_entries_like(&second));
    assert!(
        !first.plans_entries_like(&fewer),
        "a layout with other slots plans entries differently"
    );
}

#[test]
fn a_layout_with_another_eager_set_plans_entries_differently() {
    let fixture = Fixture::new();
    let selection = fixture.selection(&[]);
    let eager_handler = ComponentDescriptor {
        construction: crate::ScopedConstruction::Eager,
        ..fixture.handler
    };

    let on_demand = fixture
        .inner(&selection, &[fixture.handler, fixture.consumer])
        .expect("the inner scope plans");
    let eager = fixture
        .inner(&selection, &[eager_handler, fixture.consumer])
        .expect("the inner scope plans with an eager handler");

    assert!(!on_demand.plans_entries_like(&eager));
}

#[test]
fn entry_reads_hold_every_component_target_sorted_once() {
    let fixture = Fixture::new();
    let selection = fixture.selection(&[]);
    let layout = fixture
        .inner(&selection, &[fixture.handler, fixture.consumer])
        .expect("the inner scope plans");
    let config = DependencyDescriptor {
        config: true,
        ..eager::<Seed>("Seed", Cardinality::One)
    };
    let targets = [
        eager::<Consumer>("Consumer", Cardinality::One),
        lazy::<Handler>("Handler"),
        eager::<Consumer>("Again", Cardinality::One),
        config,
    ];
    let mut expected = vec![TypeId::of::<Consumer>(), TypeId::of::<Handler>()];

    let reads = layout.entry_reads(&plan(&layout, &selection, &targets), &targets);

    expected.sort_unstable();

    assert_eq!(
        reads.as_ref(),
        expected.as_slice(),
        "config targets select no provider, and no built slot resolves an edge through \
         its own handle"
    );
}
