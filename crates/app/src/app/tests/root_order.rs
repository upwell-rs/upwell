//! The root construction order reuses the effective graph's singleton order.

use super::*;

/// A factory-less singleton that nothing seeds.
struct Unseeded;

test_component!(Unseeded, "unseeded");

static UNSEEDED: ComponentDescriptor = ComponentDescriptor::of::<Unseeded>();

/// A factory-backed singleton declared before the unseeded singleton it requires.
struct UnseededConsumer;

test_component!(UnseededConsumer, "unseeded_consumer");

async fn unseeded_consumer(_: Arc<Unseeded>) -> UnseededConsumer {
    UnseededConsumer
}

static UNSEEDED_CONSUMER: ComponentDescriptor = ComponentDescriptor::of::<UnseededConsumer>()
    .with_factories(factories![default "static" => unseeded_consumer])
    .without_retention();

static INDEPENDENT_FACTORY_CALLS: AtomicUsize = AtomicUsize::new(0);

/// A factory-backed singleton with no dependencies, ordered before the unseeded one.
struct Independent;

test_component!(Independent, "independent");

async fn independent() -> Independent {
    INDEPENDENT_FACTORY_CALLS.fetch_add(1, Ordering::SeqCst);

    Independent
}

static INDEPENDENT: ComponentDescriptor = ComponentDescriptor::of::<Independent>()
    .with_factories(factories![default "static" => independent])
    .without_retention();

#[tokio::test]
async fn unseeded_factoryless_dependency_is_rejected_before_any_factory_runs() {
    let error = App::<()>::builder("unseeded-root-order")
        .config_source(ConfigManager::<Toml>::empty())
        .component_descriptor(&INDEPENDENT)
        .component_descriptor(&UNSEEDED_CONSUMER)
        .component_descriptor(&UNSEEDED)
        .prepare()
        .expect("an unseeded singleton is only detected at construction")
        .build()
        .await
        .expect_err("an unseeded factory-less singleton fails the root build");

    assert!(
        matches!(
            error,
            crate::Error::Di(upwell_di::Error::MissingComponent("Unseeded"))
        ),
        "expected the unseeded singleton to be reported missing, got {error:?}"
    );
    assert_eq!(INDEPENDENT_FACTORY_CALLS.load(Ordering::SeqCst), 0);
}
