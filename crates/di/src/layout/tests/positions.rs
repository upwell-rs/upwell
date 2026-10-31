//! Planned edges keep their declared positions and are read by them.

use super::*;
use crate::descriptors::BoxedComponent;
use crate::descriptors::component::{ScopeStore, SlotStore};

#[test]
fn plan_leaves_unplanned_edges_to_the_dynamic_path() {
    let fixture = Fixture::new();
    let lazy = scoped_component!(Elsewhere, "lazy-consumer", Inner::KEY, || vec![lazy::<
        Handler,
    >(
        "Handler"
    )]);
    let selection = fixture.selection(&[lazy]);

    let layout = fixture
        .inner(&selection, &[fixture.handler, lazy])
        .expect("the inner scope plans");

    assert!(
        matches!(sources(&layout, 2)[..], [DepRef::Unplanned]),
        "a lazy edge keeps its position but resolves through its own handle"
    );
}

#[test]
fn planned_edges_are_read_by_declared_position() {
    let fixture = Fixture::new();
    let mixed = scoped_component!(Elsewhere, "mixed-consumer", Inner::KEY, || vec![
        lazy::<Handler>("Handler"),
        eager::<Handler>("Handler", Cardinality::One),
    ]);
    let selection = fixture.selection(&[mixed]);
    let handler = TypeId::of::<Handler>();

    let layout = fixture
        .inner(&selection, &[fixture.handler, mixed])
        .expect("the inner scope plans");
    let planned = &layout.slots()[2].dependencies;

    assert!(
        matches!(
            PlannedDependency::at(planned, 1, handler, Cardinality::One),
            Some(DepRef::Local(at)) if at.slot == SlotIdx::new(1)
        ),
        "the eager edge after a lazy one is read at its declared position"
    );
    assert!(
        PlannedDependency::at(planned, 0, handler, Cardinality::One).is_none(),
        "an unplanned position falls back to the edge's own handle"
    );
    assert!(
        PlannedDependency::at(planned, 2, handler, Cardinality::One).is_none(),
        "a position past the declared edges has no planned source"
    );
    assert!(
        matches!(
            PlannedDependency::find(planned, handler, None, Cardinality::One),
            Some(DepRef::Local(_))
        ),
        "a typed lookup skips the unplanned edge of the same type"
    );
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "requested as another type than it declared")]
fn a_planned_edge_read_as_another_type_is_a_bug() {
    let fixture = Fixture::new();
    let selection = fixture.selection(&[]);

    let layout = fixture
        .inner(&selection, &[fixture.handler, fixture.consumer])
        .expect("the inner scope plans");

    PlannedDependency::at(
        &layout.slots()[2].dependencies,
        0,
        TypeId::of::<Seed>(),
        Cardinality::One,
    );
}

#[test]
fn a_slot_rejects_an_instance_of_another_component() {
    let fixture = Fixture::new();
    let selection = fixture.selection(&[]);

    let layout = fixture
        .inner(&selection, &[fixture.handler])
        .expect("the inner scope plans");
    let mut store = ScopeStore::Slots(SlotStore::new(Arc::new(layout)));
    let built = BoxedComponent {
        ty: TypeDescriptor::of::<Consumer>("Consumer"),
        value: Box::new(()),
    };

    let rejected = store
        .insert_at(SlotIdx::new(1), built)
        .expect_err("the handler slot holds only a handler");

    assert!(matches!(
        rejected,
        Error::FactoryOutputMismatch {
            component: "handler",
            built: "Consumer",
        }
    ));
    assert_eq!(store.component_count(), 0, "nothing was stored");
}
