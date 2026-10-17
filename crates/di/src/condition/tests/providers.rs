//! Provider fallback, ordering and dependency validation after condition filtering.

use super::*;

static DEFAULT_BEFORE_CUSTOM: [ProviderOrder; 1] = [ProviderOrder {
    target: TypeDescriptor::of::<CustomAuthenticator>("CustomAuthenticator"),
    traits: &[TypeDescriptor::of::<dyn Authenticator>("Authenticator")],
    direction: crate::ProviderOrderDirection::Before,
}];

#[test]
fn provider_fallback_uses_existing_selection_after_filtering() {
    let default = component::<DefaultAuthenticator>("default-authenticator", factory);
    let custom = component::<CustomAuthenticator>("custom-authenticator", factory)
        .with_condition(&CUSTOM_ENABLED);
    let providers = vec![
        provider::<DefaultAuthenticator>(
            TypeDescriptor::of::<dyn Authenticator>("Authenticator"),
            "default",
            false,
        ),
        provider::<CustomAuthenticator>(
            TypeDescriptor::of::<dyn Authenticator>("Authenticator"),
            "custom",
            true,
        ),
    ];
    let registry = ComponentRegistry {
        components: vec![custom, default],
        providers,
    };
    let catalog = ConditionCatalog::new(&registry, facts()).expect("catalog validates");

    let disabled = catalog
        .evaluate(&snapshot(false, "safe"))
        .expect("disabled facts evaluate");
    let enabled = catalog
        .evaluate(&snapshot(true, "safe"))
        .expect("enabled facts evaluate");

    assert!(!disabled.component_eligible("custom-authenticator").unwrap());
    assert_eq!(disabled.eligible_registry().providers.len(), 1);
    assert_eq!(
        disabled.eligible_registry().providers[0].qualifier,
        "default"
    );
    assert_eq!(enabled.eligible_registry().providers.len(), 2);

    let enabled_components = enabled
        .eligible_registry()
        .resolved_components()
        .expect("effective components resolve");
    let selection = enabled
        .eligible_registry()
        .provider_selection_model(&enabled_components)
        .expect("existing provider selection accepts effective catalog");

    assert_eq!(
        selection
            .select_runtime_one(
                TypeDescriptor::of::<dyn Authenticator>("Authenticator").type_id,
                None,
                upwell_core::ResolutionMode::Eager,
                &Singleton,
                &|_, _| true,
            )
            .expect("custom is selected")
            .provider
            .qualifier,
        "custom"
    );
}

#[test]
fn ordering_constraints_to_inactive_components_do_not_invalidate_the_candidate() {
    let default = component::<DefaultAuthenticator>("default-authenticator", factory);
    let custom = component::<CustomAuthenticator>("custom-authenticator", factory)
        .with_condition(&CUSTOM_ENABLED);
    let mut default_provider = provider::<DefaultAuthenticator>(
        TypeDescriptor::of::<dyn Authenticator>("Authenticator"),
        "default",
        false,
    );
    default_provider.ordering = &DEFAULT_BEFORE_CUSTOM;
    let registry = ComponentRegistry {
        components: vec![default, custom],
        providers: vec![
            default_provider,
            provider::<CustomAuthenticator>(
                TypeDescriptor::of::<dyn Authenticator>("Authenticator"),
                "custom",
                true,
            ),
        ],
    };
    let catalog = ConditionCatalog::new(&registry, facts()).expect("catalog validates");

    let disabled = catalog
        .evaluate_validated(&snapshot(false, "safe"))
        .expect("inactive ordering target is ignored after complete-catalog validation");
    let enabled = catalog
        .evaluate_validated(&snapshot(true, "safe"))
        .expect("active ordering target retains complete-catalog ordering");

    assert_eq!(disabled.registry().providers.len(), 1);
    assert_eq!(enabled.registry().providers.len(), 2);
}

#[test]
fn inactive_dependencies_are_ignored_but_active_dependencies_still_validate() {
    let mut conditional = component::<CustomAuthenticator>("custom-authenticator", factory)
        .with_condition(&CUSTOM_ENABLED);
    conditional.factories = missing_factory;
    let registry = ComponentRegistry {
        components: vec![conditional],
        providers: Vec::new(),
    };
    let catalog = ConditionCatalog::new(&registry, facts()).expect("catalog validates");

    catalog
        .evaluate_validated(&snapshot(false, "safe"))
        .expect("inactive ordinary dependencies are outside the effective graph");
    let events = crate::test_support::capture_events(|| {
        assert!(matches!(
            catalog.evaluate_validated(&snapshot(true, "safe")),
            Err(ConditionError::Registry(
                crate::Error::MissingDependency { .. }
            ))
        ));
    });

    assert!(events.iter().any(|event| {
        event
            .fields
            .get("event_name")
            .is_some_and(|value| value == "condition-validation")
            && event
                .fields
                .get("result")
                .is_some_and(|value| value == "rejected")
    }));
    assert!(!events.iter().any(|event| {
        event
            .fields
            .values()
            .any(|value| value == "committed" || value == "activated")
    }));
}
