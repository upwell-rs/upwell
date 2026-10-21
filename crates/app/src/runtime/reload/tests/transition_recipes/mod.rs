//! Transition-local one-shot factories, explicit hot updates, and every strategy
//! together through the transactional reload.

mod fixture;
mod hot_update;
mod matrix;
mod one_shot;

use crate::AppRuntime;

/// Writes `label` and reloads, expecting the reload to commit.
async fn relabel(dir: &tempfile::TempDir, runtime: &AppRuntime, label: &str) {
    fixture::write_recipe_config(&fixture::config_dir_of(dir), label);

    let report = runtime.reload_config().await.expect("the reload commits");

    assert!(report.is_committed(), "relabelling to {label} publishes");
}
