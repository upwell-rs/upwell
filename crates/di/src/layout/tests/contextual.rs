//! The accessed set: the slots of a scope invocation-context edges anywhere read, declared by
//! every entry without being built.

use super::*;
use crate::layout::UnplannedConstruction;

/// A root singleton reading the inner `Handler` through `Current<Arc<Handler>>`.
struct Reader;

/// An inner component reading the inner `Consumer` through `Current<Arc<Consumer>>`.
struct Watcher;

fn current<T: 'static>(name: &'static str) -> DependencyDescriptor {
    dependency(
        name,
        TypeDescriptor::of::<T>(name),
        Cardinality::One,
        ResolutionMode::Current,
    )
}

fn reader() -> ComponentDescriptor {
    component::<Reader>(
        "reader",
        unbuilt_factories!("reader", || vec![current::<Handler>("Handler")]),
    )
}

fn watcher() -> ComponentDescriptor {
    scoped_component!(Watcher, "watcher", Inner::KEY, || vec![
        current::<Consumer>("Consumer")
    ])
}

fn slot(layout: &SlotLayout, ty: TypeId) -> usize {
    layout
        .slot_of(ty)
        .expect("the inner scope holds the type")
        .index()
}

fn plan(layout: &SlotLayout, selection: &ProviderSelectionModel) -> EntryPlan {
    layout.plan_entry(
        &[eager::<Shared>("Shared", Cardinality::One)],
        selection,
        reach,
        UnplannedConstruction::Deny,
    )
}

#[test]
fn every_entry_declares_what_an_accessor_anywhere_reads_without_building_it() {
    let fixture = Fixture::new();
    let selection = fixture.selection(&[reader()]);
    let layout = fixture
        .inner(&selection, &[fixture.handler, fixture.consumer])
        .expect("the inner scope plans");

    let plan = plan(&layout, &selection);

    assert!(
        layout.is_accessed(),
        "the root reader's accessor reads the scope"
    );
    assert!(
        plan.is_empty(),
        "the accessed handler is not built up front"
    );
    assert!(plan.is_accessed(), "the entry opens its scope at once");
    assert!(plan.declared()[slot(&layout, TypeId::of::<Handler>())]);
    assert!(
        plan.reads_seed(&layout, TypeId::of::<Seed>()),
        "the seed the handler reads is captured"
    );
    assert!(layout.eager_set_reads_seed(TypeId::of::<Seed>()));
    assert!(layout.factories_read_seed(TypeId::of::<Seed>()));
    assert!(!plan.declared()[slot(&layout, TypeId::of::<Consumer>())]);
}

#[test]
fn a_contextual_edge_is_declared_but_never_built_with_its_consumer() {
    let fixture = Fixture::new();
    let selection = fixture.selection(&[watcher()]);
    let layout = fixture
        .inner(&selection, &[fixture.handler, fixture.consumer, watcher()])
        .expect("the inner scope plans");
    let consumer = slot(&layout, TypeId::of::<Consumer>());
    let watcher = slot(&layout, TypeId::of::<Watcher>());

    let plan = layout.plan_entry(
        &[eager::<Watcher>("Watcher", Cardinality::One)],
        &selection,
        reach,
        UnplannedConstruction::Deny,
    );

    assert!(matches!(
        sources(&layout, watcher)[..],
        [DepRef::Contextual]
    ));
    assert!(plan.builds(SlotIdx::new(watcher)));
    assert!(
        !plan.builds(SlotIdx::new(consumer)),
        "the accessed slot stays lazy"
    );
    assert!(plan.declared()[consumer], "the accessed slot is declared");
}

#[test]
fn a_layout_no_accessor_reads_is_unchanged() {
    let fixture = Fixture::new();
    let selection = fixture.selection(&[]);
    let layout = fixture
        .inner(&selection, &[fixture.handler, fixture.consumer])
        .expect("the inner scope plans");

    let plan = plan(&layout, &selection);

    assert!(!layout.is_accessed());
    assert!(!plan.is_accessed());
    assert!(!layout.eager_set_reads_seed(TypeId::of::<Seed>()));
    assert!(
        (0..layout.len()).all(|index| plan.declared()[index] == plan.builds(SlotIdx::new(index))),
        "the entry declares only what it builds"
    );
}

#[test]
fn layouts_with_other_accessed_sets_plan_entries_differently() {
    let fixture = Fixture::new();
    let order = [fixture.handler, fixture.consumer];
    let plain = fixture.selection(&[]);
    let read = fixture.selection(&[reader()]);

    let without = fixture
        .inner(&plain, &order)
        .expect("the inner scope plans");
    let with = fixture.inner(&read, &order).expect("the inner scope plans");

    assert!(!without.plans_entries_like(&with));
    assert!(!with.plans_entries_like(&without));
}
