use upwell_core::{Scope, ScopeId, Singleton, Transient};

use super::*;

const HTTP_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/http-request");
const CONNECTION_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/websocket-connection");
const MESSAGE_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/websocket-message");
const SIBLING_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/sibling");
const MISSING_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/missing");
const CYCLE_A_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/cycle-a");
const CYCLE_B_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/cycle-b");
const LONG_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/long");
const LEAF_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/leaf");

struct Http;
struct Connection;
struct Message;
struct Sibling;
struct CycleA;
struct CycleB;
struct Long;
struct Leaf;

macro_rules! test_scope {
    ($type:ty, $id:expr, $name:literal, $rank:expr) => {
        impl Scope for $type {
            const ID: ScopeId = $id;
            const RANK: u8 = $rank;
            const NAME: &'static str = $name;
        }
    };
}

test_scope!(Http, HTTP_ID, "Request", 100);
test_scope!(Connection, CONNECTION_ID, "Connection", 150);
test_scope!(Message, MESSAGE_ID, "Request", 50);
test_scope!(Sibling, SIBLING_ID, "Request", 50);
test_scope!(CycleA, CYCLE_A_ID, "Cycle A", 120);
test_scope!(CycleB, CYCLE_B_ID, "Cycle B", 110);
test_scope!(Long, LONG_ID, "Too Long", u8::MAX);
test_scope!(Leaf, LEAF_ID, "Leaf", 10);

static AXUM_BOUNDARIES: [ScopeBoundary; 3] = [
    ScopeBoundary::new::<Message>(&[ScopeParent::Boundary(CONNECTION_ID)]),
    ScopeBoundary::new::<Http>(&[ScopeParent::Root]),
    ScopeBoundary::new::<Connection>(&[ScopeParent::Root]),
];

fn prepare(boundaries: &'static [ScopeBoundary]) -> PreparedScopeTopology {
    ScopeTopology::new(boundaries)
        .prepare()
        .expect("valid test topology")
}

#[test]
fn declaration_is_const_and_preparation_canonicalizes_boundaries() {
    let declaration = ScopeTopology::new(&AXUM_BOUNDARIES);
    let topology = declaration.prepare().expect("valid topology");
    let declared_ids: Vec<_> = declaration
        .boundaries()
        .iter()
        .map(|boundary| boundary.id())
        .collect();
    let ids: Vec<_> = topology
        .boundaries()
        .iter()
        .map(|boundary| boundary.id())
        .collect();
    let mut expected = vec![HTTP_ID, CONNECTION_ID, MESSAGE_ID];

    expected.sort();

    assert_eq!(declared_ids, [MESSAGE_ID, HTTP_ID, CONNECTION_ID]);
    assert_eq!(ids, expected);
}

#[test]
fn empty_topology_is_const_constructible_and_default() {
    const EMPTY: ScopeTopology = ScopeTopology::empty();

    assert!(EMPTY.boundaries().is_empty());
    assert!(ScopeTopology::default().boundaries().is_empty());
}

#[test]
fn duplicate_display_names_do_not_alias_stable_ids() {
    static BOUNDARIES: [ScopeBoundary; 2] = [
        ScopeBoundary::new::<Http>(&[ScopeParent::Root]),
        ScopeBoundary::new::<Message>(&[ScopeParent::Root]),
    ];

    let topology = prepare(&BOUNDARIES);

    assert_eq!(Http::NAME, Message::NAME);
    assert_ne!(HTTP_ID, MESSAGE_ID);
    assert_eq!(
        topology.boundary(&HTTP_ID).map(|boundary| boundary.id()),
        Some(HTTP_ID)
    );
    assert_eq!(
        topology.boundary(&MESSAGE_ID).map(|boundary| boundary.id()),
        Some(MESSAGE_ID)
    );
}

#[test]
fn boundary_lookup_and_chain_queries_include_implicit_root() {
    let topology = prepare(&AXUM_BOUNDARIES);
    let root = Singleton::ID;

    assert!(topology.contains(&HTTP_ID));
    assert!(!topology.contains(&root));
    assert_eq!(
        topology.parents_of(&HTTP_ID),
        Some(&[ScopeParent::Root][..])
    );
    assert_eq!(
        topology.parents_of(&MESSAGE_ID),
        Some(&[ScopeParent::Boundary(CONNECTION_ID)][..])
    );
    assert_eq!(topology.parents_of(&MISSING_ID), None);
    assert_eq!(
        topology.chains(&MESSAGE_ID),
        [Box::from([CONNECTION_ID, root])]
    );
    assert!(topology.chains(&MISSING_ID).is_empty());
    assert!(topology.chains(&root).is_empty());
}

#[test]
fn reachability_follows_ancestry_instead_of_rank() {
    static BOUNDARIES: [ScopeBoundary; 4] = [
        ScopeBoundary::new::<Http>(&[ScopeParent::Root]),
        ScopeBoundary::new::<Connection>(&[ScopeParent::Root]),
        ScopeBoundary::new::<Message>(&[ScopeParent::Boundary(CONNECTION_ID)]),
        ScopeBoundary::new::<Sibling>(&[ScopeParent::Root]),
    ];

    let topology = prepare(&BOUNDARIES);
    let root = Singleton::ID;

    assert!(topology.is_reachable(&MESSAGE_ID, &MESSAGE_ID));
    assert!(topology.is_reachable(&MESSAGE_ID, &CONNECTION_ID));
    assert!(topology.is_reachable(&MESSAGE_ID, &root));
    assert!(topology.is_reachable(&root, &root));
    assert!(!topology.is_reachable(&root, &MESSAGE_ID));
    assert!(!topology.is_reachable(&HTTP_ID, &CONNECTION_ID));
    assert!(!topology.is_reachable(&MESSAGE_ID, &SIBLING_ID));
    assert!(!topology.is_reachable(&MISSING_ID, &root));
    assert!(!topology.is_reachable(&MESSAGE_ID, &MISSING_ID));
}

#[test]
fn every_declared_parent_contributes_an_opening_chain() {
    static BOUNDARIES: [ScopeBoundary; 3] = [
        ScopeBoundary::new::<Connection>(&[ScopeParent::Root]),
        ScopeBoundary::new::<Message>(&[ScopeParent::of::<Connection>(), ScopeParent::Root]),
        ScopeBoundary::new::<Leaf>(&[ScopeParent::of::<Message>()]),
    ];

    let topology = prepare(&BOUNDARIES);
    let root = Singleton::ID;
    let message = topology.boundary(&MESSAGE_ID).expect("message is declared");

    assert!(message.opens_under(ScopeParent::Root));
    assert!(message.opens_under(ScopeParent::of::<Connection>()));
    assert!(!message.opens_under(ScopeParent::of::<Leaf>()));
    assert_eq!(
        topology.chains(&MESSAGE_ID),
        [Box::from([CONNECTION_ID, root]), Box::from([root])]
    );
    assert_eq!(
        topology.chains(&LEAF_ID),
        [
            Box::from([MESSAGE_ID, CONNECTION_ID, root]),
            Box::from([MESSAGE_ID, root])
        ]
    );
    assert_eq!(topology.common_ancestors(&LEAF_ID), {
        let mut common = vec![MESSAGE_ID, root];

        common.sort();

        common
    });
}

#[test]
fn reachability_requires_an_ancestor_on_every_chain() {
    static BOUNDARIES: [ScopeBoundary; 3] = [
        ScopeBoundary::new::<Connection>(&[ScopeParent::Root]),
        ScopeBoundary::new::<Message>(&[ScopeParent::of::<Connection>(), ScopeParent::Root]),
        ScopeBoundary::new::<Leaf>(&[ScopeParent::of::<Message>()]),
    ];

    let topology = prepare(&BOUNDARIES);
    let root = Singleton::ID;

    assert!(topology.is_reachable(&LEAF_ID, &MESSAGE_ID));
    assert!(topology.is_reachable(&LEAF_ID, &root));
    assert!(topology.is_reachable(&MESSAGE_ID, &root));
    assert!(!topology.is_reachable(&MESSAGE_ID, &CONNECTION_ID));
    assert!(!topology.is_reachable(&LEAF_ID, &CONNECTION_ID));
}

#[test]
fn empty_and_repeated_parent_lists_are_rejected() {
    static EMPTY_BOUNDARIES: [ScopeBoundary; 1] = [ScopeBoundary::new::<Http>(&[])];
    static REPEATED_BOUNDARIES: [ScopeBoundary; 1] = [ScopeBoundary::new::<Http>(&[
        ScopeParent::Root,
        ScopeParent::Root,
    ])];

    let empty = ScopeTopology::new(&EMPTY_BOUNDARIES)
        .prepare()
        .expect_err("a boundary needs an opening parent");
    let repeated = ScopeTopology::new(&REPEATED_BOUNDARIES)
        .prepare()
        .expect_err("a parent may be listed once");

    assert_eq!(empty, ScopeTopologyError::NoParents { id: HTTP_ID });
    assert_eq!(
        repeated,
        ScopeTopologyError::DuplicateParent {
            id: HTTP_ID,
            parent: Singleton::ID,
        }
    );
}

#[test]
fn every_declared_parent_must_outrank_the_child() {
    static BOUNDARIES: [ScopeBoundary; 2] = [
        ScopeBoundary::new::<Sibling>(&[ScopeParent::Root]),
        ScopeBoundary::new::<Message>(&[ScopeParent::Root, ScopeParent::of::<Sibling>()]),
    ];

    let error = ScopeTopology::new(&BOUNDARIES)
        .prepare()
        .expect_err("the equal-rank second parent is invalid");

    assert_eq!(
        error,
        ScopeTopologyError::InvalidParentRank {
            child: MESSAGE_ID,
            child_rank: 50,
            parent: SIBLING_ID,
            parent_rank: 50,
        }
    );
}

#[test]
fn duplicate_ids_are_rejected() {
    static BOUNDARIES: [ScopeBoundary; 2] = [
        ScopeBoundary::new::<Http>(&[ScopeParent::Root]),
        ScopeBoundary::new::<Http>(&[ScopeParent::Root]),
    ];

    let error = ScopeTopology::new(&BOUNDARIES)
        .prepare()
        .expect_err("duplicate ids are invalid");

    assert_eq!(error, ScopeTopologyError::DuplicateId { id: HTTP_ID });
}

#[test]
fn root_and_transient_ids_are_reserved() {
    static ROOT_BOUNDARIES: [ScopeBoundary; 1] =
        [ScopeBoundary::new::<Singleton>(&[ScopeParent::Root])];
    static TRANSIENT_BOUNDARIES: [ScopeBoundary; 1] =
        [ScopeBoundary::new::<Transient>(&[ScopeParent::Root])];

    let root = ScopeTopology::new(&ROOT_BOUNDARIES)
        .prepare()
        .expect_err("root cannot be declared");
    let transient = ScopeTopology::new(&TRANSIENT_BOUNDARIES)
        .prepare()
        .expect_err("transient cannot be declared");

    assert_eq!(root, ScopeTopologyError::ReservedId { id: Singleton::ID });
    assert_eq!(
        transient,
        ScopeTopologyError::ReservedId { id: Transient::ID }
    );
}

#[test]
fn missing_and_self_parents_are_rejected() {
    static MISSING_BOUNDARIES: [ScopeBoundary; 1] =
        [ScopeBoundary::new::<Http>(&[ScopeParent::Boundary(
            MISSING_ID,
        )])];
    static SELF_BOUNDARIES: [ScopeBoundary; 1] =
        [ScopeBoundary::new::<Http>(&[ScopeParent::Boundary(
            HTTP_ID,
        )])];

    let missing = ScopeTopology::new(&MISSING_BOUNDARIES)
        .prepare()
        .expect_err("missing parent is invalid");
    let self_parent = ScopeTopology::new(&SELF_BOUNDARIES)
        .prepare()
        .expect_err("self-parent is invalid");

    assert_eq!(
        missing,
        ScopeTopologyError::MissingParent {
            id: HTTP_ID,
            parent: MISSING_ID,
        }
    );
    assert_eq!(self_parent, ScopeTopologyError::SelfParent { id: HTTP_ID });
}

#[test]
fn cycles_are_reported_before_impossible_rank_ordering() {
    static BOUNDARIES: [ScopeBoundary; 2] = [
        ScopeBoundary::new::<CycleB>(&[ScopeParent::Boundary(CYCLE_A_ID)]),
        ScopeBoundary::new::<CycleA>(&[ScopeParent::Boundary(CYCLE_B_ID)]),
    ];

    let error = ScopeTopology::new(&BOUNDARIES)
        .prepare()
        .expect_err("parent cycle is invalid");

    assert_eq!(
        error,
        ScopeTopologyError::Cycle {
            members: Box::new([CYCLE_A_ID, CYCLE_B_ID]),
        }
    );
}

#[test]
fn parent_must_have_strictly_greater_lifetime_rank() {
    static EQUAL_RANK_BOUNDARIES: [ScopeBoundary; 2] = [
        ScopeBoundary::new::<Message>(&[ScopeParent::Boundary(SIBLING_ID)]),
        ScopeBoundary::new::<Sibling>(&[ScopeParent::Root]),
    ];
    static ROOT_RANK_BOUNDARIES: [ScopeBoundary; 1] =
        [ScopeBoundary::new::<Long>(&[ScopeParent::Root])];

    let equal = ScopeTopology::new(&EQUAL_RANK_BOUNDARIES)
        .prepare()
        .expect_err("equal parent rank is invalid");
    let root = ScopeTopology::new(&ROOT_RANK_BOUNDARIES)
        .prepare()
        .expect_err("root rank must be greater than child rank");

    assert_eq!(
        equal,
        ScopeTopologyError::InvalidParentRank {
            child: MESSAGE_ID,
            child_rank: 50,
            parent: SIBLING_ID,
            parent_rank: 50,
        }
    );
    assert_eq!(
        root,
        ScopeTopologyError::InvalidParentRank {
            child: LONG_ID,
            child_rank: u8::MAX,
            parent: Singleton::ID,
            parent_rank: u8::MAX,
        }
    );
}
