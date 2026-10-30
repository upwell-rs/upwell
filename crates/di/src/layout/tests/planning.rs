//! The source each dependency edge plans to, and the edges planning rejects.

use super::*;

#[test]
fn plan_lays_out_seeds_first_and_indexes_every_slot() {
    let fixture = Fixture::new();
    let selection = fixture.selection(&[]);

    let layout = fixture
        .inner(&selection, &[fixture.handler, fixture.consumer])
        .expect("the inner scope plans");
    let ids: Vec<&str> = layout.components().map(|component| component.id).collect();

    assert_eq!(
        ids,
        ["seed", "handler", "consumer"],
        "seeds precede factory-backed slots"
    );
    assert!(
        layout.slots()[0].factory.is_none(),
        "a seed slot has no factory"
    );
    assert_eq!(
        layout.slot_of(TypeId::of::<Consumer>()),
        Some(SlotIdx::new(2))
    );
    assert_eq!(
        layout.providers_of(marker().type_id).len(),
        1,
        "only the inner provider of Marker is aliased in the inner scope"
    );
}

#[test]
fn plan_resolves_ancestor_and_local_edges_to_their_slots() {
    let fixture = Fixture::new();
    let selection = fixture.selection(&[]);

    let layout = fixture
        .inner(&selection, &[fixture.handler, fixture.consumer])
        .expect("the inner scope plans");
    let handler = sources(&layout, 1);

    assert!(
        matches!(handler[0], DepRef::Ancestor { layout, at } if layout.scope().id() == Outer::ID && at.slot == SlotIdx::new(0) && at.alias.is_none()),
        "the outer connection is read from the outer layout's first slot, got {:?}",
        handler[0]
    );
    assert!(
        matches!(handler[1], DepRef::Local(at) if at.slot == SlotIdx::new(0)),
        "the seed is read from the inner seed slot, got {:?}",
        handler[1]
    );
}

#[test]
fn plan_resolves_transient_provider_and_collection_edges() {
    let fixture = Fixture::new();
    let selection = fixture.selection(&[]);

    let layout = fixture
        .inner(&selection, &[fixture.handler, fixture.consumer])
        .expect("the inner scope plans");
    let consumer = sources(&layout, 2);

    assert!(
        matches!(consumer[0], DepRef::Local(at) if at.slot == SlotIdx::new(1) && at.alias.is_none())
    );
    assert!(
        matches!(consumer[1], DepRef::Transient(plan) if matches!(plan.dependencies[0].source, DepRef::Root(RootTarget::Component(_)))),
        "a transient is planned with its own root edge, got {:?}",
        consumer[1]
    );
    assert!(
        matches!(consumer[2], DepRef::Local(at) if at.slot == SlotIdx::new(1) && at.alias == Some(0)),
        "the nearest Marker provider is the inner handler's first alias, got {:?}",
        consumer[2]
    );

    let DepRef::Many(provided) = consumer[3] else {
        panic!(
            "a collection edge plans every provider, got {:?}",
            consumer[3]
        );
    };
    let qualifiers: Vec<&str> = provided.iter().map(|entry| entry.qualifier).collect();

    assert_eq!(
        qualifiers,
        ["handler", "shared"],
        "providers keep provider order"
    );
    assert!(matches!(
        provided[1].source,
        DepRef::Root(RootTarget::Provider(_))
    ));
}

#[test]
fn plan_marks_an_unsatisfied_optional_edge_absent() {
    let fixture = Fixture::new();
    let optional = scoped_component!(Elsewhere, "optional-consumer", Inner::KEY, || {
        let mut edge = eager::<Sibling>("Sibling", Cardinality::One);

        edge.optional = true;

        vec![edge]
    });
    let selection = fixture.selection(&[optional]);

    let layout = fixture
        .inner(&selection, &[optional])
        .expect("the inner scope plans");

    assert!(matches!(sources(&layout, 1)[0], DepRef::Absent));
}

#[test]
fn plan_rejects_a_dependency_held_by_no_scope_on_the_chain() {
    let fixture = Fixture::new();
    let elsewhere = scoped_component!(Elsewhere, "elsewhere", Sibling::KEY, Vec::new);
    let consumer = scoped_component!(Consumer, "sibling-consumer", Inner::KEY, || vec![eager::<
        Elsewhere,
    >(
        "Elsewhere",
        Cardinality::One
    )]);
    let mut fixture = fixture;

    fixture.consumer = consumer;

    let selection = fixture.selection(&[elsewhere]);
    let error = fixture
        .inner(&selection, &[consumer])
        .expect_err("a sibling scope's component has no slot on the chain");

    assert!(
        matches!(&error, Error::UnplacedDependency { component_id, target, scope, .. } if component_id == "sibling-consumer" && target == "elsewhere" && *scope == "Sibling"),
        "the error names the consumer, target and scope, got {error:?}"
    );
}
