use std::any::TypeId;
use std::sync::Arc;

use upwell_core::Transient;

use super::*;
use crate::{Cardinality, from_boxed};

#[test]
fn a_single_recipe_is_named_by_its_path_and_effective() {
    let factory = ENGLISH
        .effective_factory()
        .expect("one recipe is unambiguous")
        .expect("the recipe is declared");

    assert_eq!(ENGLISH.id, "english");
    assert_eq!(ENGLISH.name, "English");
    assert_eq!(factory.id, "factory:english");
    assert!(!factory.default);
    assert!(ENGLISH.dependencies().is_empty());
    assert_eq!(
        ENGLISH.condition.map(|condition| condition.id),
        Some("greeter-enabled")
    );
    assert!(
        ENGLISH.generation_snapshot.is_some(),
        "built from `of`, the descriptor stays retainable"
    );
}

#[test]
fn the_explicit_recipe_wins_over_the_declared_default() {
    let factory = ENGLISH_RECIPES
        .effective_factory()
        .expect("one explicit recipe is unambiguous")
        .expect("a recipe is declared");
    let dependencies = ENGLISH_RECIPES.dependencies();

    assert_eq!((ENGLISH_RECIPES.factories)().len(), 2);
    assert_eq!(factory.id, "from-swedish");
    assert_eq!(
        dependencies.len(),
        1,
        "the recipe's parameters are its dependencies"
    );
    assert_eq!(dependencies[0].cardinality, Cardinality::One);
    assert_eq!(dependencies[0].ty.type_id, TypeId::of::<Swedish>());
}

#[test]
fn setters_override_one_field_each() {
    let descriptor = ComponentDescriptor::of::<Swedish>()
        .with_scope(&Transient)
        .with_hooks(upwell_hooks::no_hooks)
        .without_retention();

    assert_eq!(descriptor.scope.id(), Transient.id());
    assert!(descriptor.generation_snapshot.is_none());
    assert!(descriptor.condition.is_none());
    assert!(
        (descriptor.factories)().is_empty(),
        "no recipe until one is declared"
    );
}

#[test]
fn a_provider_defaults_to_the_component_id_and_overrides_by_setter() {
    let plain = ProviderDescriptor::of::<English, dyn Greeter>("dyn Greeter");
    let tuned = plain.primary().qualifier("en").priority(-1);

    assert_eq!(plain.trait_ty.type_id, TypeId::of::<dyn Greeter>());
    assert_eq!(plain.trait_ty.name, "dyn Greeter");
    assert_eq!(plain.concrete_ty.type_id, TypeId::of::<English>());
    assert_eq!(plain.qualifier, "english");
    assert!(!plain.primary);
    assert_eq!(plain.priority, 0);
    assert!(plain.ordering.is_empty());
    assert!(tuned.primary);
    assert_eq!(tuned.qualifier, "en");
    assert_eq!(tuned.priority, -1);
}

#[test]
fn erase_provider_stores_the_instance_under_its_trait() {
    let provider = ProviderDescriptor::of::<Swedish, dyn Greeter>("dyn Greeter");
    let concrete = BoxedComponent::new(Swedish);
    let erased = (provider.erase)(&concrete);
    let greeter = from_boxed::<Arc<dyn Greeter>>(&erased).expect("stored under the trait");

    assert_eq!(concrete.ty.type_id, TypeId::of::<Swedish>());
    assert_eq!(erased.ty.type_id, TypeId::of::<dyn Greeter>());
    assert_eq!(greeter.greet(), "hej");
}
