//! Factory recipe identity, explicit-over-default selection, and the replacement
//! factory contract.

use super::*;
use crate::test_support::unbuilt_recipe;

use ComponentFactoryRole::{Default, Explicit, HotUpdate, Replacement, Transition};

/// Resolves a registry holding one component built by `factories`.
fn resolve(
    factories: fn() -> &'static [ComponentFactoryDescriptor],
) -> crate::Result<Vec<ComponentDescriptor>> {
    ComponentRegistry {
        components: vec![descriptor_with_factories(factories)],
        providers: Vec::new(),
    }
    .resolved_components()
}

/// One required eager edge on marker type `T`, named `name`.
fn edge<T: 'static>(name: &'static str) -> Vec<DependencyDescriptor> {
    vec![dependency(
        name,
        TypeDescriptor::of::<T>(name),
        Cardinality::One,
        ResolutionMode::Eager,
    )]
}

fn one_u8() -> Vec<DependencyDescriptor> {
    edge::<u8>("u8")
}

fn one_u16() -> Vec<DependencyDescriptor> {
    edge::<u16>("u16")
}

fn optional_u8() -> Vec<DependencyDescriptor> {
    edge::<u8>("u8")
        .into_iter()
        .map(|dependency| DependencyDescriptor {
            optional: true,
            ..dependency
        })
        .collect()
}

fn lazy_u8() -> Vec<DependencyDescriptor> {
    edge::<u8>("u8")
        .into_iter()
        .map(|dependency| DependencyDescriptor {
            resolution: ResolutionMode::Lazy,
            ..dependency
        })
        .collect()
}

#[test]
fn factory_ids_are_validated_during_registry_resolution() {
    static EMPTY: [ComponentFactoryDescriptor; 1] = [unbuilt_recipe("", Explicit, no_dependencies)];
    static DUPLICATE: [ComponentFactoryDescriptor; 2] = [
        unbuilt_recipe("duplicate", Explicit, no_dependencies),
        unbuilt_recipe("duplicate", Default, no_dependencies),
    ];

    let empty_error = resolve(|| &EMPTY).expect_err("empty factory ID is invalid");
    let duplicate_error = resolve(|| &DUPLICATE).expect_err("duplicate factory ID is invalid");

    assert!(matches!(empty_error, Error::EmptyFactoryId(_)));
    assert!(matches!(duplicate_error, Error::DuplicateFactoryId { .. }));
}

#[test]
fn explicit_factory_still_wins_over_multiple_defaults() {
    static FACTORIES: [ComponentFactoryDescriptor; 3] = [
        unbuilt_recipe("default-a", Default, no_dependencies),
        unbuilt_recipe("explicit", Explicit, no_dependencies),
        unbuilt_recipe("default-b", Default, no_dependencies),
    ];

    let resolved = resolve(|| &FACTORIES)
        .expect("multiple defaults remain valid when one explicit factory exists");

    assert_eq!(
        resolved[0]
            .effective_factory()
            .expect("factory selection succeeds")
            .expect("explicit factory exists")
            .id,
        "explicit"
    );
}

#[test]
fn replacement_factories_never_take_part_in_ordinary_selection() {
    static FACTORIES: [ComponentFactoryDescriptor; 2] = [
        unbuilt_recipe("replace", Replacement, one_u8),
        unbuilt_recipe("default", Default, one_u8),
    ];

    let resolved = resolve(|| &FACTORIES).expect("a replacement beside its default is valid");
    let descriptor = resolved[0];

    assert_eq!(
        descriptor
            .effective_factory()
            .expect("factory selection succeeds")
            .expect("the default factory exists")
            .id,
        "default"
    );
    assert_eq!(
        descriptor
            .replacement_factory()
            .expect("replacement selection succeeds")
            .expect("the replacement factory exists")
            .id,
        "replace"
    );
}

#[test]
fn the_replacement_contract_is_validated_during_registry_resolution() {
    static AMBIGUOUS: [ComponentFactoryDescriptor; 3] = [
        unbuilt_recipe("default", Default, no_dependencies),
        unbuilt_recipe("first", Replacement, no_dependencies),
        unbuilt_recipe("second", Replacement, no_dependencies),
    ];
    static ORPHANED: [ComponentFactoryDescriptor; 1] =
        [unbuilt_recipe("replace", Replacement, no_dependencies)];
    static UNDECLARED: [ComponentFactoryDescriptor; 2] = [
        unbuilt_recipe("default", Default, one_u8),
        unbuilt_recipe("replace", Replacement, one_u16),
    ];

    let ambiguous = resolve(|| &AMBIGUOUS).expect_err("two replacements are ambiguous");
    let orphaned = resolve(|| &ORPHANED).expect_err("a replacement needs an ordinary factory");
    let undeclared = resolve(|| &UNDECLARED).expect_err("a replacement cannot widen the graph");

    assert!(matches!(
        ambiguous,
        Error::AmbiguousTransitionFactory {
            role: Replacement,
            ..
        }
    ));
    assert!(matches!(
        orphaned,
        Error::TransitionRecipeWithoutFactory {
            role: Replacement,
            ..
        }
    ));
    assert!(matches!(
        undeclared,
        Error::UndeclaredTransitionDependency { role: Replacement, ref dependency, .. }
            if dependency == "u16"
    ));
}

#[test]
fn transition_handlers_and_hot_updates_share_the_replacement_contract() {
    static AMBIGUOUS: [ComponentFactoryDescriptor; 3] = [
        unbuilt_recipe("default", Default, no_dependencies),
        unbuilt_recipe("first", Transition, no_dependencies),
        unbuilt_recipe("second", Transition, no_dependencies),
    ];
    static ORPHANED: [ComponentFactoryDescriptor; 1] =
        [unbuilt_recipe("handler", Transition, no_dependencies)];
    static UNDECLARED: [ComponentFactoryDescriptor; 2] = [
        unbuilt_recipe("default", Default, one_u8),
        unbuilt_recipe("update", HotUpdate, one_u16),
    ];
    static DECLARED: [ComponentFactoryDescriptor; 3] = [
        unbuilt_recipe("default", Default, one_u8),
        unbuilt_recipe("handler", Transition, no_dependencies),
        unbuilt_recipe("update", HotUpdate, one_u8),
    ];

    let ambiguous = resolve(|| &AMBIGUOUS).expect_err("two transition handlers are ambiguous");
    let orphaned = resolve(|| &ORPHANED).expect_err("a handler needs an ordinary factory");
    let undeclared = resolve(|| &UNDECLARED).expect_err("a hot update cannot widen the graph");

    resolve(|| &DECLARED).expect("declared transition recipes validate");

    assert!(matches!(
        ambiguous,
        Error::AmbiguousTransitionFactory {
            role: Transition,
            ..
        }
    ));
    assert!(matches!(
        orphaned,
        Error::TransitionRecipeWithoutFactory {
            role: Transition,
            ..
        }
    ));
    assert!(matches!(
        undeclared,
        Error::UndeclaredTransitionDependency { role: HotUpdate, ref dependency, .. }
            if dependency == "u16"
    ));
}

#[test]
fn transition_recipes_are_singleton_only() {
    static RECIPES: [ComponentFactoryDescriptor; 4] = [
        unbuilt_recipe("default", Default, no_dependencies),
        unbuilt_recipe("replace", Replacement, no_dependencies),
        unbuilt_recipe("handler", Transition, no_dependencies),
        unbuilt_recipe("update", HotUpdate, no_dependencies),
    ];

    let transient = ComponentRegistry {
        components: vec![descriptor_with_factories(|| &RECIPES).with_scope(Transient::KEY)],
        providers: Vec::new(),
    }
    .resolved_components()
    .expect_err("a transient component cannot declare transition recipes");

    resolve(|| &RECIPES).expect("a singleton declares every recipe");

    assert!(matches!(
        transient,
        Error::TransitionRecipeOutOfScope {
            role: Replacement,
            scope: "Transient",
            ..
        }
    ));
}

#[test]
fn a_replacement_edge_may_be_no_stricter_than_its_ordinary_edge() {
    static RELAXED: [ComponentFactoryDescriptor; 2] = [
        unbuilt_recipe("default", Default, one_u8),
        unbuilt_recipe("replace", Replacement, optional_u8),
    ];
    static TIGHTENED: [ComponentFactoryDescriptor; 2] = [
        unbuilt_recipe("default", Default, optional_u8),
        unbuilt_recipe("replace", Replacement, one_u8),
    ];
    static EAGER_OVER_LAZY: [ComponentFactoryDescriptor; 2] = [
        unbuilt_recipe("default", Default, lazy_u8),
        unbuilt_recipe("replace", Replacement, one_u8),
    ];

    resolve(|| &RELAXED).expect("a replacement may treat a required edge as optional");

    let tightened = resolve(|| &TIGHTENED).expect_err("an optional edge cannot become required");
    let eager = resolve(|| &EAGER_OVER_LAZY).expect_err("a lazy edge cannot be resolved eagerly");

    assert!(matches!(
        tightened,
        Error::UndeclaredTransitionDependency { .. }
    ));
    assert!(matches!(
        eager,
        Error::UndeclaredTransitionDependency { .. }
    ));
}
