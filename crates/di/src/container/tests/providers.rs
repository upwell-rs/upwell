//! Provider indexing, selection and registry ordering.

use super::*;

static CONCRETE_ID_CALLS: AtomicUsize = AtomicUsize::new(0);

fn counted_concrete_id() -> TypeId {
    CONCRETE_ID_CALLS.fetch_add(1, Ordering::Relaxed);

    TypeId::of::<u8>()
}

#[test]
fn provider_lookup_uses_the_prebuilt_concrete_index() {
    const PROVIDERS: usize = 256;

    let concrete_ty = TypeDescriptor {
        name: "Counted",
        type_name: std::any::type_name::<u8>,
        type_id: counted_concrete_id(),
        source: None,
    };
    let provider = ProviderDescriptor {
        trait_ty: TypeDescriptor::of::<dyn Send>("dyn Send"),
        concrete_ty,
        qualifier: "counted",
        primary: false,
        priority: 0,
        ordering: &[],
        erase: unerased,
        live: |_| None,
    };
    let component = ComponentDescriptor::manual("counted", "Counted", concrete_ty, Singleton::KEY);
    let registry = ScopeRegistry::new(
        HashMap::new(),
        HashMap::from([(component.ty.type_id, component)]),
        vec![provider; PROVIDERS],
        HashMap::new(),
    )
    .expect("provider component is registered");

    CONCRETE_ID_CALLS.store(0, Ordering::SeqCst);

    for _ in 0..1_000 {
        assert_eq!(registry.providers_for(TypeId::of::<u8>()).len(), PROVIDERS);
    }

    assert_eq!(
        CONCRETE_ID_CALLS.load(Ordering::SeqCst),
        0,
        "provider lookup rescanned concrete descriptor functions"
    );
}

fn test_provider(name: &'static str, qualifier: &'static str, primary: bool) -> ProviderDescriptor {
    planned_provider(
        TypeDescriptor::of::<dyn Send>("dyn Send"),
        TypeDescriptor::of::<u8>(name),
        qualifier,
        primary,
    )
}

#[test]
fn single_provider_selection_is_shared_and_ambiguity_aware() {
    let sole = [test_provider("Sole", "sole", false)];

    assert_eq!(
        select_single_provider(&sole).map(|p| p.qualifier),
        Some("sole")
    );

    let two_plain = [
        test_provider("First", "first", false),
        test_provider("Second", "second", false),
    ];

    assert!(select_single_provider(&two_plain).is_none());

    let unique_primary = [
        test_provider("First", "first", true),
        test_provider("Second", "second", false),
    ];

    assert_eq!(
        select_single_provider(&unique_primary).map(|p| p.qualifier),
        Some("first")
    );

    let two_primaries = [
        test_provider("First", "first", true),
        test_provider("Second", "second", true),
    ];

    assert!(select_single_provider(&two_primaries).is_none());
}

#[test]
fn registry_construction_completes_missing_provider_ordinals() {
    let provider = test_provider("Unplanned", "unplanned", false);
    let component = ComponentDescriptor::manual(
        "unplanned",
        "Unplanned",
        provider.concrete_ty,
        Singleton::KEY,
    );
    let registry = ScopeRegistry::new(
        HashMap::new(),
        HashMap::from([(component.ty.type_id, component)]),
        vec![provider],
        HashMap::new(),
    )
    .expect("provider component is registered");

    // Previously this expect() panicked for providers missing from the plan.
    assert_eq!(registry.provider_ordinal(&provider), 0);
}

#[test]
fn registry_indexes_follow_final_provider_order_for_first_wins_lookups() {
    let first = ProviderDescriptor {
        trait_ty: TypeDescriptor::of::<dyn Send>("dyn Send"),
        concrete_ty: TypeDescriptor::of::<u8>("First"),
        qualifier: "shared",
        primary: false,
        priority: 20,
        ordering: &[],
        erase: unerased,
        live: |_| None,
    };
    let winning = ProviderDescriptor {
        trait_ty: TypeDescriptor::of::<dyn Send>("dyn Send"),
        concrete_ty: TypeDescriptor::of::<u16>("Winning"),
        qualifier: "shared",
        primary: false,
        priority: -10,
        ordering: &[],
        erase: unerased,
        live: |_| None,
    };
    let order = HashMap::from([(
        TypeId::of::<dyn Send>(),
        HashMap::from([(TypeId::of::<u8>(), 1), (TypeId::of::<u16>(), 0)]),
    )]);
    let components = [first, winning]
        .into_iter()
        .map(|provider| {
            let component = ComponentDescriptor::manual(
                provider.qualifier,
                provider.concrete_ty.name,
                provider.concrete_ty,
                Singleton::KEY,
            );

            (component.ty.type_id, component)
        })
        .collect();
    let registry = ScopeRegistry::new(HashMap::new(), components, vec![first, winning], order)
        .expect("provider components are registered");

    assert_eq!(
        registry
            .selection
            .select_global(TypeId::of::<dyn Send>(), Some("shared"))
            .map(|provider| provider.concrete_ty.type_id),
        Some(TypeId::of::<u16>())
    );
    assert_eq!(
        registry
            .selection
            .providers_for_trait(TypeId::of::<dyn Send>())
            .iter()
            .map(|provider| provider.concrete_ty.type_id)
            .collect::<Vec<_>>(),
        [TypeId::of::<u16>(), TypeId::of::<u8>()]
    );
}

#[test]
fn registry_construction_rejects_orphan_provider() {
    let provider = test_provider("Orphan", "orphan", false);
    let error = match ScopeRegistry::new(
        HashMap::new(),
        HashMap::new(),
        vec![provider],
        HashMap::new(),
    ) {
        Ok(_) => panic!("orphan provider must be rejected"),
        Err(error) => error,
    };

    assert!(matches!(error, Error::ProviderComponentMissing(_)));
}
