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
        &Singleton,
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
        &Singleton,
    );
    let registry = ComponentRegistry {
        components: vec![PG_POOL, BACKUP_REPO, manual],
        ..Default::default()
    };

    let resolved = registry.resolved_components().expect("resolve components");
    let ids: Vec<_> = resolved.iter().map(|component| component.id).collect();

    assert_eq!(ids, ["pg_pool_manual", "backup_repo"]);
}
