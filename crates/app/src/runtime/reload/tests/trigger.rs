//! Automatic reload triggers drive the runtime through [`ReloadTarget`], so a triggered
//! reload transitions the component graph instead of taking the config-only path.

use upwell_config::ReloadTarget;

use super::fixture::{
    ProbeComponent, build_probe_app, config_dir_of, factory_calls, lock_test_guard, reset_controls,
    write_probe_config,
};

#[tokio::test]
async fn triggered_reload_publishes_a_runtime_generation() {
    let _guard = lock_test_guard().await;

    reset_controls().await;

    let (dir, app) = build_probe_app(false, 1).await;
    let runtime = app.runtime();
    let old_generation = runtime.generation();
    let old_config_generation = app.runtime().config_generation();

    write_probe_config(&config_dir_of(&dir), true, 3);

    let summary = runtime
        .trigger_reload()
        .await
        .expect("the triggered reload succeeds");

    assert_eq!(summary.changed, 1, "only the probe binding changed");
    assert_eq!(
        summary.generation,
        old_config_generation + 1,
        "the summary reports the committed config generation"
    );
    assert_eq!(
        runtime.generation().get(),
        old_generation.get() + 1,
        "the trigger transitions the runtime generation, not only config"
    );

    let probe = runtime
        .root()
        .get::<ProbeComponent>()
        .expect("the condition-activated component resolves from the new root");

    assert_eq!(probe.observed, 3, "the candidate saw the triggered config");
    assert_eq!(factory_calls(), 1, "the candidate factory ran exactly once");
}

#[tokio::test]
async fn triggered_reload_watches_the_reloader_sources() {
    let _guard = lock_test_guard().await;

    reset_controls().await;

    let (dir, app) = build_probe_app(false, 1).await;
    let runtime = app.runtime();

    let sources = ReloadTarget::sources(runtime);

    assert!(
        sources
            .iter()
            .any(|source| source.starts_with(config_dir_of(&dir))),
        "the probe config file is among the watched sources"
    );
}
