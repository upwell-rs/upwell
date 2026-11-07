//! How a `#[scope]` declaration joins the application's topology.

#![allow(dead_code)]

use upwell::daemon::App;
use upwell::scope::Root;
use upwell::{
    AppError, Descriptor, Scope, ScopeDeclaration, ScopeGenerations, ScopeId, ScopeParent,
    ScopeTopologyError, scope,
};

use super::{Batch, CustomerSync};

/// A scope declared by hand, without `#[scope]`, under [`Batch`].
struct HandWritten;

impl Scope for HandWritten {
    const ID: ScopeId = upwell::namespaced_id!(ScopeId, "test/hand_written");
    const RANK: u8 = 10;
    const NAME: &'static str = "HandWritten";
}

/// A scope claiming the identity the RPC protocol's request scope already has.
struct RequestAlias;

impl Scope for RequestAlias {
    const ID: ScopeId = upwell::scope::Request::ID;
    const RANK: u8 = 10;
    const NAME: &'static str = "RequestAlias";
}

/// A scope declared through the macro with every path spelled through an override.
#[scope(parents = [fork::scope::Root], upwell = ::fork, id = "test/renamed", rank = 7, name = "Renamed run")]
struct Renamed;

#[test]
fn the_declaration_derives_identity_rank_and_parameters_from_the_type() {
    let declaration = <CustomerSync as Descriptor<ScopeDeclaration>>::DESCRIPTOR;
    let source = declaration
        .source
        .expect("the macro records the declaration");

    assert_eq!(CustomerSync::ID.as_str(), "app/customer_sync");
    assert_eq!(CustomerSync::NAME, "CustomerSync");
    assert_eq!(CustomerSync::RANK, u8::MAX - 1);
    assert_eq!(declaration.boundary.parents(), [ScopeParent::Root]);
    assert_eq!(declaration.fields, ["customer"]);
    assert!(declaration.params.is_some());
    assert_eq!(source.item, "CustomerSync");
}

#[test]
fn a_unit_scope_stores_no_parameters_and_keeps_its_declared_policy() {
    let declaration = <Batch as Descriptor<ScopeDeclaration>>::DESCRIPTOR;

    assert_eq!(Batch::RANK, CustomerSync::RANK - 1);
    assert_eq!(Batch::GENERATIONS, ScopeGenerations::Durable);
    assert_eq!(
        declaration.boundary.parents(),
        [ScopeParent::of::<CustomerSync>()]
    );
    assert!(declaration.params.is_none());
    assert!(declaration.fields.is_empty());
}

#[test]
fn discovered_scopes_join_the_protocol_topology_once() {
    let prepared = App::builder("user-scopes-topology")
        .scope::<CustomerSync>()
        .auto_discover()
        .prepare()
        .expect("the application prepares");
    let topology = prepared.scope_topology();
    let declared = prepared
        .registry()
        .scopes
        .iter()
        .filter(|scope| scope.boundary.id() == CustomerSync::ID)
        .count();

    assert!(topology.contains(&CustomerSync::ID));
    assert!(topology.is_reachable(&Batch::ID, &CustomerSync::ID));
    assert_eq!(declared, 1);
}

#[test]
fn an_undiscovered_scope_is_not_declared() {
    let prepared = App::builder("user-scopes-undiscovered")
        .prepare()
        .expect("the application prepares");

    assert!(!prepared.scope_topology().contains(&CustomerSync::ID));
}

/// The topology error preparing `declaration` alone fails with.
fn topology_error(name: &str, declaration: ScopeDeclaration) -> ScopeTopologyError {
    let result = App::builder(name).scope_declaration(declaration).prepare();

    match result {
        Ok(_) => panic!("the declaration is rejected"),
        Err(upwell::daemon::Error::App(error)) => match *error {
            AppError::ScopeTopology(error) => error,
            other => panic!("expected a topology error, got {other}"),
        },
        Err(other) => panic!("expected an application error, got {other}"),
    }
}

#[test]
fn a_parent_the_topology_lacks_is_a_typed_error() {
    static PARENTS: [ScopeParent; 1] = [ScopeParent::of::<Batch>()];

    let error = topology_error(
        "user-scopes-orphan",
        ScopeDeclaration::new::<HandWritten>(&PARENTS),
    );

    assert!(
        matches!(error, ScopeTopologyError::MissingParent { .. }),
        "{error}"
    );
}

#[test]
fn an_identity_the_protocol_already_uses_is_a_typed_error() {
    static PARENTS: [ScopeParent; 1] = [ScopeParent::Root];

    let error = topology_error(
        "user-scopes-duplicate",
        ScopeDeclaration::new::<RequestAlias>(&PARENTS),
    );

    assert!(
        matches!(error, ScopeTopologyError::DuplicateId { .. }),
        "{error}"
    );
}

#[test]
fn the_path_override_and_explicit_constants_reach_the_scope() {
    assert_eq!(Renamed::ID.as_str(), "test/renamed");
    assert_eq!(Renamed::NAME, "Renamed run");
    assert_eq!(Renamed::RANK, 7);
    assert_eq!(ScopeParent::of::<Root>(), ScopeParent::Root);
}
