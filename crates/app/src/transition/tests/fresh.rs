//! Factory-backed transients stay Fresh-resolvable in candidate registries.

use super::*;
use upwell_core::Scope;

#[derive(Clone)]
struct TransientSeed;

impl Injectable for TransientSeed {
    type Target = Self;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

#[derive(Clone)]
struct FreshHolder {
    fresh: Fresh<TransientSeed>,
}

impl Injectable for FreshHolder {
    type Target = Self;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

fn construct_transient_seed(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<TransientSeed>("TransientSeed"),
            value: Box::new(Injectable::into_stored(TransientSeed)),
        })
    })
}

fn construct_fresh_holder(
    cx: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        let fresh = <Fresh<TransientSeed> as FromContainer>::from_container(cx).await?;

        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<FreshHolder>("FreshHolder"),
            value: Box::new(Injectable::into_stored(std::sync::Arc::new(FreshHolder {
                fresh,
            }))),
        })
    })
}

static TRANSIENT_SEED_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "seed",
    construct: construct_transient_seed,
    dependencies: no_dependencies,
    role: ComponentFactoryRole::Explicit,
}];
static ACTIVE_HOLDER_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "holder-active",
    construct: construct_fresh_holder,
    dependencies: no_dependencies,
    role: ComponentFactoryRole::Explicit,
}];
static CANDIDATE_HOLDER_FACTORY: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "holder-candidate",
    construct: construct_fresh_holder,
    dependencies: no_dependencies,
    role: ComponentFactoryRole::Explicit,
}];

fn transient_seed_factories() -> &'static [ComponentFactoryDescriptor] {
    &TRANSIENT_SEED_FACTORY
}

fn active_holder_factories() -> &'static [ComponentFactoryDescriptor] {
    &ACTIVE_HOLDER_FACTORY
}

fn candidate_holder_factories() -> &'static [ComponentFactoryDescriptor] {
    &CANDIDATE_HOLDER_FACTORY
}

fn fresh_holder(factories: fn() -> &'static [ComponentFactoryDescriptor]) -> ComponentDescriptor {
    ComponentDescriptor {
        id: "fresh-holder",
        name: "FreshHolder",
        ty: TypeDescriptor::of::<FreshHolder>("FreshHolder"),
        scope: Singleton::KEY,
        condition: None,
        factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    }
}

fn transient_seed() -> ComponentDescriptor {
    ComponentDescriptor {
        id: "transient-seed",
        name: "TransientSeed",
        ty: TypeDescriptor::of::<TransientSeed>("TransientSeed"),
        scope: Transient::KEY,
        condition: None,
        factories: transient_seed_factories,
        hooks: upwell_hooks::no_hooks,
        generation_snapshot: None,
    }
}

#[tokio::test]
async fn candidate_registry_keeps_factory_backed_transients_fresh_resolvable() {
    let (active_graph, candidate) = graphs(
        &registry(
            [transient_seed(), fresh_holder(active_holder_factories)],
            [],
        ),
        &registry(
            [transient_seed(), fresh_holder(candidate_holder_factories)],
            [],
        ),
    );
    let resolved = candidate
        .resolve_transition(
            &active_graph,
            [ComponentTransitionDecision {
                component: "fresh-holder",
                strategy: ComponentTransitionStrategy::Reconstruct,
            }],
        )
        .expect("holder reconstruction is supported");

    let (root, _) = resolved
        .build_candidate_root(&candidate, Vec::new(), Vec::new(), ResolverSet::new())
        .await
        .expect("resolved plan constructs candidate root");
    let holder = root
        .resolve::<std::sync::Arc<FreshHolder>>()
        .await
        .expect("fresh holder resolves from the candidate root")
        .expect("fresh holder is stored in the candidate root");

    holder
        .fresh
        .create()
        .await
        .expect("factory-backed transient stays Fresh-resolvable after the transition");
}
