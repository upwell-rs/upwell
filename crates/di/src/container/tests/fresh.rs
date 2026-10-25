//! Fresh resolution against declared scope ancestry and the requesting scope.

use super::*;

#[tokio::test]
async fn fresh_target_resolves_from_its_declared_scope_ancestry() {
    let target_provider = ComponentDescriptor {
        id: "target-provider",
        name: "TargetProvider",
        ty: TypeDescriptor::of::<TargetProvider>("TargetProvider"),
        scope: FreshTargetScope::KEY,
        condition: None,
        factories: target_provider_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    };
    let owner_provider = ComponentDescriptor {
        id: "owner-provider",
        name: "OwnerProvider",
        ty: TypeDescriptor::of::<OwnerProvider>("OwnerProvider"),
        scope: FreshOwnerScope::KEY,
        condition: None,
        factories: owner_provider_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    };
    let fresh_target = ComponentDescriptor {
        id: "fresh-target",
        name: "FreshTarget",
        ty: TypeDescriptor::of::<FreshTarget>("FreshTarget"),
        scope: FreshTargetScope::KEY,
        condition: None,
        factories: fresh_target_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    };
    let target_seed = ComponentDescriptor::manual(
        "target-seed",
        "TargetSeed",
        TypeDescriptor::of::<TargetSeed>("TargetSeed"),
        FreshTargetScope::KEY,
    );
    let components = [target_provider, owner_provider, fresh_target, target_seed]
        .into_iter()
        .map(|descriptor| (descriptor.ty.type_id, descriptor))
        .collect();
    let providers = vec![
        provider_descriptor(target_provider.ty, erase_target_provider),
        provider_descriptor(owner_provider.ty, erase_owner_provider),
    ];
    let registry = Arc::new(
        ScopeRegistry::new(HashMap::new(), components, providers, HashMap::new())
            .expect("registry validates"),
    );
    let root =
        ScopeContainer::build_root(&[], Vec::new(), ResolverSet::new(), Arc::clone(&registry))
            .await
            .expect("root builds");
    let seed = boxed_component("TargetSeed", Arc::new(TargetSeed("target-seed")));
    let target_scope = ScopeContainer::open_child(
        FreshTargetScope::KEY,
        root,
        Arc::clone(&registry),
        &[target_seed, target_provider],
        vec![seed],
    )
    .await
    .expect("target scope opens");
    let owner_scope = ScopeContainer::open_child(
        FreshOwnerScope::KEY,
        target_scope,
        Arc::clone(&registry),
        &[owner_provider],
        Vec::new(),
    )
    .await
    .expect("owner scope opens");

    let boxed = construct_fresh_boxed(&registry, owner_scope, fresh_target)
        .await
        .expect("fresh target constructs");
    let target = from_boxed::<Arc<FreshTarget>>(&boxed).expect("fresh target stored");

    assert_eq!(
        target.provider.source(),
        "target",
        "the owner's shorter-lived provider must not leak into fresh construction"
    );
    assert_eq!(
        target.seed.0, "target-seed",
        "a dynamic seed in the target's declared scope remains accessible"
    );
}

#[tokio::test]
async fn fresh_reconstructs_a_transient_against_the_requesting_scope() {
    struct RootBound;

    #[derive(Clone)]
    struct TransientLeaf {
        bound: Arc<RootBound>,
    }

    impl Injectable for TransientLeaf {
        type Target = Self;
        type Stored = Self;

        fn into_stored(self) -> Self {
            self
        }

        fn from_stored(stored: &Self) -> Self {
            stored.clone()
        }
    }

    fn transient_leaf_factory(
        cx: &mut ComponentConstructionContext,
    ) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + '_>> {
        Box::pin(async {
            let bound = cx
                .resolve::<Arc<RootBound>>()
                .await?
                .ok_or(Error::MissingComponent("RootBound"))?;

            Ok(BoxedComponent {
                ty: TypeDescriptor::of::<TransientLeaf>("TransientLeaf"),
                value: Box::new(Injectable::into_stored(TransientLeaf { bound })),
            })
        })
    }

    static TRANSIENT_LEAF_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
        id: "static",
        construct: transient_leaf_factory,
        dependencies: no_dependencies,
        role: ComponentFactoryRole::Explicit,
    }];

    let bound = ComponentDescriptor::manual(
        "root-bound",
        "RootBound",
        TypeDescriptor::of::<RootBound>("RootBound"),
        Singleton::KEY,
    );
    let transient_leaf = ComponentDescriptor {
        id: "transient-leaf",
        name: "TransientLeaf",
        ty: TypeDescriptor::of::<TransientLeaf>("TransientLeaf"),
        scope: Transient::KEY,
        condition: None,
        factories: || &TRANSIENT_LEAF_FACTORY,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    };
    let components = [(bound.ty.type_id, bound)].into_iter().collect();
    let transients = [(transient_leaf.ty.type_id, transient_leaf)]
        .into_iter()
        .collect();
    let registry = Arc::new(
        ScopeRegistry::new(transients, components, Vec::new(), HashMap::new())
            .expect("registry validates"),
    );
    let bound = Arc::new(RootBound);
    let root = ScopeContainer::build_root(
        &[],
        vec![boxed_component("RootBound", Arc::clone(&bound))],
        ResolverSet::new(),
        Arc::clone(&registry),
    )
    .await
    .expect("root builds");

    let boxed = construct_fresh_boxed(&registry, root, transient_leaf)
        .await
        .expect("a factory-backed transient reconstructs against the requesting scope");
    let leaf = from_boxed::<TransientLeaf>(&boxed).expect("transient leaf stored");

    assert!(
        Arc::ptr_eq(&leaf.bound, &bound),
        "the rebuilt transient resolves its dependency up the requesting scope's chain"
    );
}
