//! Registry validation of fulfilled, duplicate, missing and manual components.

use super::*;

static PG_POOL: ComponentDescriptor = ComponentDescriptor {
    name: "PgPool",
    ..planned(
        "pg_pool",
        TypeDescriptor::of::<u16>("PgPool"),
        unbuilt_factories!("static", no_dependencies),
    )
};

static BACKUP_REPO: ComponentDescriptor = ComponentDescriptor {
    name: "BackupRepository",
    ..planned(
        "backup_repo",
        TypeDescriptor::of::<u8>("BackupRepository"),
        unbuilt_factories!("static", || {
            vec![dependency(
                "PgPool",
                TypeDescriptor::of::<u16>("PgPool"),
                Cardinality::One,
                ResolutionMode::Eager,
            )]
        }),
    )
};

#[test]
fn validate_passes_with_fulfilled_dependencies() {
    let registry = ComponentRegistry {
        components: vec![BACKUP_REPO, PG_POOL],
        ..Default::default()
    };

    assert!(registry.validate().is_ok());
}

#[test]
fn validate_detects_duplicate_component_ids() {
    let registry = ComponentRegistry {
        components: vec![BACKUP_REPO, BACKUP_REPO],
        ..Default::default()
    };

    assert!(registry.validate().is_err());
}

#[test]
fn validate_detects_missing_dependency() {
    let registry = ComponentRegistry {
        components: vec![BACKUP_REPO],
        ..Default::default()
    };

    assert!(registry.validate().is_err());
}

#[test]
fn validate_accepts_manual_component_descriptor() {
    let without = ComponentRegistry {
        components: vec![BACKUP_REPO],
        ..Default::default()
    };

    assert!(without.validate().is_err());

    let manual = ComponentDescriptor::manual(
        "pg_pool_manual",
        "PgPool",
        TypeDescriptor::of::<u16>("PgPool"),
        Singleton::KEY,
    );
    let with = ComponentRegistry {
        components: vec![BACKUP_REPO, manual],
        ..Default::default()
    };

    assert!(with.validate().is_ok());
}

#[test]
fn resolved_components_preserve_registration_order_through_manual_override() {
    let manual = ComponentDescriptor::manual(
        "pg_pool_manual",
        "PgPool",
        TypeDescriptor::of::<u16>("PgPool"),
        Singleton::KEY,
    );
    let registry = ComponentRegistry {
        components: vec![PG_POOL, BACKUP_REPO, manual],
        ..Default::default()
    };

    let resolved = registry.resolved_components().expect("resolve components");
    let ids: Vec<_> = resolved.iter().map(|component| component.id).collect();

    assert_eq!(ids, ["pg_pool_manual", "backup_repo"]);
}

#[test]
fn validate_rejects_eager_components_without_a_factory() {
    let eager = ComponentDescriptor {
        construction: crate::ScopedConstruction::Eager,
        ..ComponentDescriptor::manual(
            "seeded",
            "Seeded",
            TypeDescriptor::of::<u16>("Seeded"),
            Request::KEY,
        )
    };
    let registry = ComponentRegistry {
        components: vec![eager],
        ..Default::default()
    };

    let error = registry.validate();

    assert!(
        matches!(
            error,
            Err(Error::EagerWithoutFactory { component_id }) if component_id == "seeded"
        ),
        "a factory-less eager component is rejected"
    );
}

#[test]
fn validate_rejects_eager_components_outside_scopes_below_the_root() {
    for scope in [Singleton::KEY, crate::Transient::KEY] {
        let eager = ComponentDescriptor {
            scope,
            construction: crate::ScopedConstruction::Eager,
            ..PG_POOL
        };
        let registry = ComponentRegistry {
            components: vec![eager],
            ..Default::default()
        };

        let error = registry.validate();

        assert!(
            matches!(
                error,
                Err(Error::EagerOutsideScope {
                    component_id,
                    ..
                }) if component_id == "pg_pool"
            ),
            "an eager component of {} is rejected",
            scope.name()
        );
    }
}
