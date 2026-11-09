//! One component type built in several scopes through `#[init(scope = ..)]`: the macro
//! records each factory's scope and registry resolution expands them into placements.
#![allow(dead_code)]

use upwell::{ComponentRegistry, Scope, ScopeId, component, methods};

/// The first of two disjoint scopes.
struct ScopeA;
/// The second of two disjoint scopes.
struct ScopeB;

impl Scope for ScopeA {
    const ID: ScopeId = upwell::namespaced_id!(ScopeId, "test/multi-a");
    const RANK: u8 = 1;
    const NAME: &'static str = "ScopeA";
}

impl Scope for ScopeB {
    const ID: ScopeId = upwell::namespaced_id!(ScopeId, "test/multi-b");
    const RANK: u8 = 1;
    const NAME: &'static str = "ScopeB";
}

/// A store with one constructor per scope.
#[component(scope = ScopeA)]
struct Store {
    #[default]
    source: String,
}

#[methods]
impl Store {
    #[init]
    fn in_a() -> Self {
        Self {
            source: "a".to_string(),
        }
    }
}

#[methods]
impl Store {
    #[init(scope = ScopeB)]
    fn in_b() -> Self {
        Self {
            source: "b".to_string(),
        }
    }
}

fn placements() -> Vec<(ScopeId, &'static str)> {
    let registry = ComponentRegistry::collect();
    let type_id = std::any::TypeId::of::<Store>();

    registry
        .resolved_components()
        .expect("the store resolves")
        .into_iter()
        .filter(|component| component.ty.type_id == type_id)
        .map(|component| {
            let factory = component
                .effective_factory()
                .expect("one factory per scope")
                .expect("the placement is built");

            (component.scope.id(), factory.id)
        })
        .collect()
}

#[test]
fn each_init_builds_in_the_scope_it_declares() {
    let mut placements = placements();

    placements.sort();

    let mut expected = vec![(ScopeA::ID, "init:in_a"), (ScopeB::ID, "init:in_b")];

    expected.sort();

    assert_eq!(placements, expected);
}

#[test]
fn an_init_without_a_scope_inherits_the_declared_one() {
    let in_a = placements()
        .into_iter()
        .find(|(_, factory)| *factory == "init:in_a")
        .expect("the unscoped init is registered");

    assert_eq!(in_a.0, ScopeA::ID);
}

/// A component whose field-injection default builds in both scopes.
#[component(scope = [ScopeA, ScopeB])]
struct Pair {}

#[test]
fn a_scope_list_places_the_default_factory_in_every_listed_scope() {
    let type_id = std::any::TypeId::of::<Pair>();
    let resolved = ComponentRegistry::collect()
        .resolved_components()
        .expect("the pair resolves");
    let mut placements: Vec<_> = resolved
        .iter()
        .filter(|component| component.ty.type_id == type_id)
        .map(|component| {
            let factory = component
                .effective_factory()
                .expect("one factory per scope")
                .expect("the placement is built");

            (component.scope.id(), factory.id)
        })
        .collect();
    let declared = <Pair as upwell::DeclaredScope>::Scope::ID;

    placements.sort();

    let mut expected = vec![(ScopeA::ID, "default"), (ScopeB::ID, "default")];

    expected.sort();

    assert_eq!(placements, expected);
    assert_eq!(declared, ScopeA::ID, "the first listed scope is declared");
}
