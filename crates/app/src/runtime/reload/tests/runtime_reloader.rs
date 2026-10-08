//! The [`RuntimeReloader`] handle drives the one transactional reload pipeline, survives
//! the generations it publishes, and never keeps its runtime alive.

use crate::Error;
use crate::builtins::RuntimeReloader;

use super::fixture::{
    ProbeComponent, build_probe_app, config_dir_of, lock_test_guard, reset_controls,
    write_probe_config,
};

#[tokio::test]
async fn app_reloader_publishes_a_runtime_generation() {
    let _guard = lock_test_guard().await;

    reset_controls().await;

    let (dir, app) = build_probe_app(false, 1).await;
    let runtime = app.runtime();
    let old_generation = runtime.generation();
    let old_config_generation = runtime.config_generation();

    write_probe_config(&config_dir_of(&dir), true, 5);

    let report = app
        .reloader()
        .reload()
        .await
        .expect("the reloader's reload succeeds");

    assert!(report.published, "a changed source publishes a generation");
    assert_eq!(report.runtime_generation, runtime.generation());
    assert_eq!(
        runtime.generation().get(),
        old_generation.get() + 1,
        "the reloader transitions the runtime generation"
    );
    assert_eq!(
        report.config_generation,
        old_config_generation + 1,
        "the reloader commits the config generation once"
    );

    let probe = runtime
        .root()
        .get::<ProbeComponent>()
        .expect("the condition-activated component resolves from the new root");

    assert_eq!(probe.observed, 5, "the candidate saw the reloaded config");
}

#[tokio::test]
async fn injected_reloader_survives_the_generations_it_publishes() {
    let _guard = lock_test_guard().await;

    reset_controls().await;

    let (dir, app) = build_probe_app(false, 1).await;
    let runtime = app.runtime();
    let initial = runtime.generation();
    let injected = runtime
        .root()
        .get::<RuntimeReloader>()
        .expect("the reloader is a seeded root singleton");

    write_probe_config(&config_dir_of(&dir), true, 2);

    injected.reload().await.expect("the first reload succeeds");

    let retained = runtime
        .root()
        .get::<RuntimeReloader>()
        .expect("the published root still provides the reloader");

    write_probe_config(&config_dir_of(&dir), true, 3);

    retained
        .reload()
        .await
        .expect("the handle from the published root reloads again");

    assert_eq!(
        runtime.generation().get(),
        initial.get() + 2,
        "both handles drive the same runtime"
    );

    write_probe_config(&config_dir_of(&dir), true, 4);

    injected
        .reload()
        .await
        .expect("the handle from the initial root still drives the current runtime");

    assert_eq!(runtime.generation().get(), initial.get() + 3);
}

#[tokio::test]
async fn reloader_does_not_keep_its_runtime_alive() {
    let _guard = lock_test_guard().await;

    reset_controls().await;

    let (_dir, app) = build_probe_app(false, 1).await;
    let reloader = app.reloader();

    drop(app);

    let error = reloader
        .reload()
        .await
        .expect_err("a dropped runtime cannot reload");

    assert!(
        matches!(error, Error::RuntimeUnavailable),
        "expected RuntimeUnavailable, got {error:?}"
    );
}

#[tokio::test]
async fn unattached_reloader_reports_an_unavailable_runtime() {
    let error = RuntimeReloader::new()
        .reload()
        .await
        .expect_err("an unattached reloader cannot reload");

    assert!(
        matches!(error, Error::RuntimeUnavailable),
        "expected RuntimeUnavailable, got {error:?}"
    );
}
