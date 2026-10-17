//! Preparation validates without constructing components or the protocol runtime.

use super::*;

#[tokio::test]
async fn prepare_validates_without_constructing_components_or_protocol() {
    let _boundary_guard = BOUNDARY_GUARD.lock().await;

    FACTORY_CALLS.store(0, Ordering::SeqCst);
    PRE_BUILD_CALLS.store(0, Ordering::SeqCst);
    PROTOCOL_BUILD_CALLS.store(0, Ordering::SeqCst);

    let prepared = App::<BoundaryProtocol>::builder("prepare-boundary-test")
        .config_source(
            ConfigManager::<Toml>::from_str(
                r#"
                    [logging]
                    level = "debug"
                    format = "compact"
                    ansi = false
                "#,
            )
            .expect("test config parses"),
        )
        .prepare()
        .expect("application prepares");

    assert_eq!(PRE_BUILD_CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(FACTORY_CALLS.load(Ordering::SeqCst), 0);
    assert_eq!(PROTOCOL_BUILD_CALLS.load(Ordering::SeqCst), 0);
    assert_eq!(
        std::any::type_name_of_val(prepared.protocol()),
        std::any::type_name::<PreparedBoundaryProtocol>()
    );

    let app = prepared.build().await.expect("prepared application builds");

    assert_eq!(FACTORY_CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(PROTOCOL_BUILD_CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::any::type_name_of_val(app.protocol()),
        std::any::type_name::<BoundaryRuntime>()
    );
    assert!(app.container().get::<BoundaryComponent>().is_some());
    assert!(app.container().get::<SeededComponent>().is_some());
}

#[test]
#[cfg(feature = "tooling")]
fn retained_tooling_construction_plan_equals_the_runtime_build_plan() {
    let _boundary_guard = BOUNDARY_GUARD.blocking_lock();

    let prepared = App::<BoundaryProtocol>::builder("prepare-boundary-test")
        .config_source(
            ConfigManager::<Toml>::from_str(
                r#"
                    [logging]
                    level = "debug"
                    format = "compact"
                    ansi = false
                "#,
            )
            .expect("test config parses"),
        )
        .prepare()
        .expect("application prepares");
    let runtime = prepared
        .root_order
        .iter()
        .map(|component| component.id)
        .collect::<Vec<_>>();
    let tooling = prepared
        .tooling_snapshot()
        .root_plan()
        .iter()
        .map(|entry| entry.descriptor.id)
        .collect::<Vec<_>>();

    assert_eq!(tooling, runtime);
}
