//! Serialized reload attempts commit their staged candidates in entry order.

use std::task::Poll;
use std::time::Duration;

use tokio::time::timeout;
use upwell_config::ContainerConfigExt;

use crate::RuntimeReloadReport;

use super::fixture::{
    ProbeComponent, ProbeConfig, build_probe_app, config_dir_of, factory_parked, lock_test_guard,
    park_factory_after_staging, release_factory, reset_controls, stage_entries, staged_tokens,
    write_probe_config,
};

/// Aborts a reload task and opens its factory gate if an assertion unwinds while it is parked.
struct ParkedReload {
    task: Option<tokio::task::JoinHandle<crate::Result<RuntimeReloadReport>>>,
    factory_parked: bool,
}

impl ParkedReload {
    fn spawn(runtime: crate::AppRuntime) -> Self {
        Self {
            task: Some(tokio::spawn(async move { runtime.reload_config().await })),
            factory_parked: false,
        }
    }

    fn mark_factory_parked(&mut self) {
        self.factory_parked = true;
    }

    fn release_factory(&mut self) {
        self.factory_parked = false;
        release_factory();
    }

    async fn join(&mut self) -> Result<crate::Result<RuntimeReloadReport>, tokio::task::JoinError> {
        self.task
            .take()
            .expect("reload task is joined at most once")
            .await
    }
}

impl Drop for ParkedReload {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            if self.factory_parked {
                release_factory();
            }

            task.abort();
        }
    }
}

#[tokio::test]
async fn concurrent_reloads_commit_in_staging_order() {
    let _guard = lock_test_guard().await;

    reset_controls().await;

    let (dir, app) = build_probe_app(false, 1).await;
    reset_controls().await;

    let runtime = app.runtime();
    let base_runtime_generation = runtime.generation();
    let base_config_generation = app.config_reloader().generation();

    park_factory_after_staging(true).await;
    write_probe_config(&config_dir_of(&dir), true, 2);

    let mut reload_a = ParkedReload::spawn(runtime.clone());

    timeout(Duration::from_secs(5), factory_parked())
        .await
        .expect("reload A enters candidate construction after staging V2");
    reload_a.mark_factory_parked();

    assert_eq!(stage_entries(), 1, "only reload A entered config staging");

    write_probe_config(&config_dir_of(&dir), false, 3);

    let reload_b_runtime = runtime.clone();
    let mut reload_b = std::pin::pin!(reload_b_runtime.reload_config());

    assert!(
        matches!(futures::poll!(reload_b.as_mut()), Poll::Pending),
        "reload B attempts the held config serializer and remains pending"
    );

    assert_eq!(
        stage_entries(),
        1,
        "reload B cannot enter config staging while reload A is blocked"
    );

    reload_a.release_factory();

    let report_a = timeout(Duration::from_secs(5), reload_a.join())
        .await
        .expect("reload A completes after its factory is released")
        .expect("reload A task completes")
        .expect("reload A succeeds");
    let report_b = timeout(Duration::from_secs(5), reload_b.as_mut())
        .await
        .expect("reload B completes after reload A commits")
        .expect("reload B succeeds");

    assert_eq!(
        stage_entries(),
        2,
        "reload B entered config staging after reload A committed"
    );

    assert!(report_a.published, "reload A publishes its V2 candidate");
    assert!(report_b.published, "reload B publishes its V3 candidate");
    assert_eq!(
        report_a.runtime_generation.get(),
        base_runtime_generation.get() + 1,
        "reload A advances the runtime generation once"
    );
    assert_eq!(
        report_b.runtime_generation.get(),
        base_runtime_generation.get() + 2,
        "reload B advances the runtime generation after reload A"
    );
    assert_eq!(
        report_a.config_generation,
        base_config_generation + 1,
        "reload A advances the config generation once"
    );
    assert_eq!(
        report_b.config_generation,
        base_config_generation + 2,
        "reload B advances the config generation after reload A"
    );
    assert_eq!(
        runtime.generation(),
        report_b.runtime_generation,
        "the active root belongs to the current runtime generation"
    );
    assert_eq!(
        app.config_reloader().generation(),
        report_b.config_generation,
        "the active config generation is reload B's terminal commit"
    );
    assert_eq!(
        staged_tokens(),
        vec![2, 3],
        "candidate config staging observes V2 then V3 in order"
    );
    assert_eq!(
        runtime
            .root()
            .config::<ProbeConfig>("probe")
            .expect("the probe binding resolves")
            .snapshot()
            .token,
        3,
        "reload A cannot overwrite reload B's V3 config"
    );
    assert!(
        runtime.root().get::<ProbeComponent>().is_none(),
        "the active root is reload B's V3 candidate, not reload A's V2 root"
    );

    reset_controls().await;
}
