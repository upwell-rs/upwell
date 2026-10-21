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

#[tokio::test]
async fn unseeded_factoryless_dependency_is_reported_missing() {
    let error = App::<()>::builder("unseeded-root-order")
        .config_source(ConfigManager::<Toml>::empty())
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
}
