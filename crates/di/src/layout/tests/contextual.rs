//! The slots an entry declares for the invocation-context edges it reaches.

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

#[test]
fn a_singletons_accessor_declares_its_target_without_building_it() {
    let fixture = Fixture::new();
    let selection = fixture.selection(&[reader()]);
    let layout = fixture
        .inner(&selection, &[fixture.handler, fixture.consumer])
        .expect("the inner scope plans");

    let plan = layout.plan_entry(
        &[eager::<Reader>("Reader", Cardinality::One)],
        &selection,
        reach,
        UnplannedConstruction::Deny,
    );

    assert!(
        plan.is_empty(),
        "the accessed handler is not built up front"
    );
    assert!(plan.is_accessed(), "the scope opens for the accessor");
    assert!(plan.declared()[slot(&layout, TypeId::of::<Handler>())]);
    assert!(
        plan.reads_seed(&layout, TypeId::of::<Seed>()),
        "the seed the handler reads is captured"
    );
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

    let plan = layout.plan_entry(
        &[eager::<Watcher>("Watcher", Cardinality::One)],
        &selection,
        reach,
        UnplannedConstruction::Deny,
    );

    assert!(matches!(
        sources(&layout, slot(&layout, TypeId::of::<Watcher>()))[..],
        [DepRef::Contextual]
    ));
    assert!(plan.builds(SlotIdx::new(slot(&layout, TypeId::of::<Watcher>()))));
    assert!(
        !plan.builds(SlotIdx::new(consumer)),
        "the accessed slot stays lazy"
    );
    assert!(plan.declared()[consumer], "the accessed slot is declared");
}

#[test]
fn an_entry_reaching_no_accessor_is_not_accessed() {
    let fixture = Fixture::new();
    let selection = fixture.selection(&[]);
    let layout = fixture
        .inner(&selection, &[fixture.handler, fixture.consumer])
        .expect("the inner scope plans");

    let plan = layout.plan_entry(
        &[eager::<Shared>("Shared", Cardinality::One)],
        &selection,
        reach,
        UnplannedConstruction::Deny,
    );

    assert!(!plan.is_accessed());
}
