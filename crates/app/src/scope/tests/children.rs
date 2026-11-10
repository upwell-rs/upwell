//! Application scopes that add themselves as a parent of scopes declared elsewhere.

use super::*;

const SESSION_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/session");

/// An application scope requests may open under.
struct Session;

test_scope!(Session, SESSION_ID, "Session", 200);

static SESSION_PARENTS: [ScopeParent; 1] = [ScopeParent::Root];

/// `Session`, declared with `children`.
fn session(children: &'static [ScopeId]) -> ScopeDeclaration {
    ScopeDeclaration::new::<Session>(&SESSION_PARENTS).with_children(children)
}

fn prepare_with(
    declared: &[ScopeDeclaration],
) -> Result<PreparedScopeTopology, ScopeTopologyError> {
    ScopeTopology::new(&AXUM_BOUNDARIES).prepare_with(declared)
}

#[test]
fn a_child_also_opens_under_the_scope_after_its_declared_parents() {
    let topology = prepare_with(&[session(&[HTTP_ID])]).expect("valid extended topology");

    assert_eq!(
        topology.parents_of(&HTTP_ID),
        Some(&[ScopeParent::Root, ScopeParent::Boundary(SESSION_ID)][..])
    );
    assert_eq!(topology.common_ancestors(&HTTP_ID), [Singleton::ID]);
    assert!(!topology.is_reachable(&HTTP_ID, &SESSION_ID));
    assert!(topology.encloses(&SESSION_ID, &HTTP_ID));
    assert_eq!(
        topology.parents_of(&SESSION_ID),
        Some(&[ScopeParent::Root][..])
    );
}

#[test]
fn repeated_preparations_share_one_extended_parent_list() {
    let first = prepare_with(&[session(&[HTTP_ID])]).expect("valid extended topology");
    let second = prepare_with(&[session(&[HTTP_ID])]).expect("valid extended topology");
    let first = first.parents_of(&HTTP_ID).expect("a declared boundary");
    let second = second.parents_of(&HTTP_ID).expect("a declared boundary");

    assert!(std::ptr::eq(first, second));
}

#[test]
fn rejected_preparations_intern_no_parent_list() {
    let before = interned_parent_lists();

    for count in 2..20 {
        let declared = vec![session(&[HTTP_ID]); count];
        let error = prepare_with(&declared).expect_err("a scope declared twice");

        assert_eq!(error, ScopeTopologyError::DuplicateId { id: SESSION_ID });
    }

    assert_eq!(
        interned_parent_lists(),
        before,
        "a rejected preparation keeps none of the lists it extended"
    );
}

#[test]
fn a_child_no_boundary_declares_is_rejected() {
    let error = prepare_with(&[session(&[MISSING_ID])]).expect_err("an unknown child");

    assert_eq!(
        error,
        ScopeTopologyError::MissingChild {
            id: SESSION_ID,
            child: MISSING_ID,
        }
    );
}

#[test]
fn a_child_that_already_opens_under_the_scope_is_rejected() {
    static PARENTS: [ScopeParent; 1] = [ScopeParent::Boundary(SESSION_ID)];
    static NESTED: [ScopeBoundary; 1] = [ScopeBoundary::new::<Leaf>(&PARENTS)];

    let error = ScopeTopology::new(&NESTED)
        .prepare_with(&[session(&[LEAF_ID])])
        .expect_err("a repeated parent");

    assert_eq!(
        error,
        ScopeTopologyError::DuplicateParent {
            id: LEAF_ID,
            parent: SESSION_ID,
        }
    );
}

#[test]
fn a_child_that_does_not_rank_below_the_scope_is_rejected() {
    static PARENTS: [ScopeParent; 1] = [ScopeParent::Root];

    let leaf = ScopeDeclaration::new::<Leaf>(&PARENTS).with_children(&[HTTP_ID]);
    let error = prepare_with(&[leaf]).expect_err("a child ranked above its parent");

    assert_eq!(
        error,
        ScopeTopologyError::InvalidParentRank {
            child: HTTP_ID,
            child_rank: Http::RANK,
            parent: LEAF_ID,
            parent_rank: Leaf::RANK,
        }
    );
}
