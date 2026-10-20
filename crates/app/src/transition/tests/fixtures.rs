//! Components several transition test topics share.

use super::*;

/// A singleton whose active and candidate factories label the instance they build.
pub(super) struct Replaceable {
    pub(super) label: &'static str,
}

test_component!(Replaceable, "replaceable");

async fn active_replaceable() -> Replaceable {
    Replaceable { label: "active" }
}

pub(super) async fn candidate_replaceable() -> Replaceable {
    Replaceable { label: "candidate" }
}

pub(super) const ACTIVE_FACTORIES: fn() -> &'static [ComponentFactoryDescriptor] =
    upwell_di::factories!["active" => active_replaceable];
pub(super) const CANDIDATE_FACTORIES: fn() -> &'static [ComponentFactoryDescriptor] =
    upwell_di::factories!["candidate" => candidate_replaceable];

pub(super) const fn replaceable(
    factories: fn() -> &'static [ComponentFactoryDescriptor],
) -> ComponentDescriptor {
    ComponentDescriptor::of::<Replaceable>()
        .with_factories(factories)
        .without_retention()
}

unit_component!(InvalidatedRoot, "invalidated-root", invalidated_root);
unit_component!(
    FixedDependent,
    "fixed-dependent",
    fixed_dependent,
    deps [Arc<InvalidatedRoot>]
);
unit_component!(UnrelatedSingleton, "unrelated", unrelated_singleton);

/// The trait the live providers serve and the live consumer observes.
pub(super) trait LiveService: Send + Sync {}

impl LiveService for LiveProviderA {}

impl LiveService for LiveProviderB {}

unit_component!(LiveProviderA, "live-provider-a", live_provider_a, provides dyn LiveService);
unit_component!(LiveProviderB, "live-provider-b", live_provider_b, provides dyn LiveService);
unit_component!(
    LiveConsumer,
    "live-consumer",
    live_consumer,
    deps [upwell_di::Dep<dyn LiveService>]
);

/// The live service mapping of provider `T`, qualified `"live"`. Its erase panics, so
/// a planning regression that erases a provider fails these planning-only tests.
pub(super) const fn live_service_provider<T>() -> ProviderDescriptor
where
    T: Component + upwell_di::ProviderOf<dyn LiveService>,
{
    ProviderDescriptor {
        erase: |_| panic!("transition planning must not erase components"),
        live: |_| None,
        ..ProviderDescriptor::of::<T, dyn LiveService>("dyn LiveService").qualifier("live")
    }
}

/// A retainable singleton serving `Svc`, seeded by the framework without a factory.
pub(super) trait Svc: Send + Sync {
    fn label(&self) -> &'static str;
}

pub(super) struct Retainable {
    pub(super) label: &'static str,
}

impl Svc for Retainable {
    fn label(&self) -> &'static str {
        self.label
    }
}

test_component!(Retainable, "retainable", provides dyn Svc);

/// A framework-style seed descriptor: typed identity, no factory, snapshot adapter.
pub(super) fn retainable_descriptor() -> ComponentDescriptor {
    ComponentDescriptor::manual_of::<Retainable>("retainable", "Retainable", &Singleton)
}

pub(super) fn svc_provider() -> ProviderDescriptor {
    ProviderDescriptor::of::<Retainable, dyn Svc>("dyn Svc").qualifier("svc")
}

pub(super) fn scope_registry(
    descriptors: &[ComponentDescriptor],
    providers: Vec<ProviderDescriptor>,
) -> Arc<ScopeRegistry> {
    let components = descriptors
        .iter()
        .map(|descriptor| (descriptor.ty.type_id, *descriptor))
        .collect();

    Arc::new(
        ScopeRegistry::new(HashMap::new(), components, providers, HashMap::new())
            .expect("scope registry validates"),
    )
}

pub(super) fn seed_retainable(instance: &Arc<Retainable>) -> BoxedComponent {
    BoxedComponent {
        ty: TypeDescriptor::of::<Retainable>(Retainable::NAME),
        value: Box::new(Injectable::into_stored(Arc::clone(instance))),
    }
}

pub(super) fn seed_root_resolver() -> BoxedComponent {
    BoxedComponent {
        ty: TypeDescriptor::of::<RootResolver>(RootResolver::NAME),
        value: Box::new(Injectable::into_stored(RootResolver::new())),
    }
}

pub(super) fn seed_hook_manager(hooks: &HookManager) -> BoxedComponent {
    BoxedComponent {
        ty: TypeDescriptor::of::<HookManager>(HOOK_MANAGER_NAME),
        value: Box::new(Injectable::into_stored(hooks.clone())),
    }
}
