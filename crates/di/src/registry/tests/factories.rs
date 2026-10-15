//! Factory recipe identity and explicit-over-default selection.

use super::*;

#[test]
fn factory_ids_are_validated_during_registry_resolution() {
    static EMPTY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
        id: "",
        construct: unbuilt,
        dependencies: no_dependencies,
        default: false,
    }];
    static DUPLICATE: [ComponentFactoryDescriptor; 2] = [
        ComponentFactoryDescriptor {
            id: "duplicate",
            construct: unbuilt,
            dependencies: no_dependencies,
            default: false,
        },
        ComponentFactoryDescriptor {
            id: "duplicate",
            construct: unbuilt,
            dependencies: no_dependencies,
            default: true,
        },
    ];
    fn empty() -> &'static [ComponentFactoryDescriptor] {
        &EMPTY
    }
    fn duplicate() -> &'static [ComponentFactoryDescriptor] {
        &DUPLICATE
    }

    let empty_error = ComponentRegistry {
        components: vec![descriptor_with_factories(empty)],
        providers: Vec::new(),
    }
    .resolved_components()
    .expect_err("empty factory ID is invalid");
    let duplicate_error = ComponentRegistry {
        components: vec![descriptor_with_factories(duplicate)],
        providers: Vec::new(),
    }
    .resolved_components()
    .expect_err("duplicate factory ID is invalid");

    assert!(matches!(empty_error, Error::EmptyFactoryId(_)));
    assert!(matches!(duplicate_error, Error::DuplicateFactoryId { .. }));
}

#[test]
fn explicit_factory_still_wins_over_multiple_defaults() {
    static FACTORIES: [ComponentFactoryDescriptor; 3] = [
        ComponentFactoryDescriptor {
            id: "default-a",
            construct: unbuilt,
            dependencies: no_dependencies,
            default: true,
        },
        ComponentFactoryDescriptor {
            id: "explicit",
            construct: unbuilt,
            dependencies: no_dependencies,
            default: false,
        },
        ComponentFactoryDescriptor {
            id: "default-b",
            construct: unbuilt,
            dependencies: no_dependencies,
            default: true,
        },
    ];
    fn factories() -> &'static [ComponentFactoryDescriptor] {
        &FACTORIES
    }

    let descriptor = descriptor_with_factories(factories);
    let resolved = ComponentRegistry {
        components: vec![descriptor],
        providers: Vec::new(),
    }
    .resolved_components()
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
