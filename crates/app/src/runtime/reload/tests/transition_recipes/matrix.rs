//! #295 acceptance: one transition exercising every strategy at once — reconstruction,
//! a transition handler, a replacement factory, retention with an in-place live rebind,
//! and a hot update — applies all of them together on commit, and applies none of them
//! when the attempt is rejected or cancelled after every strategy has prepared. Every
//! candidate instance built for such an attempt is dropped with it.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::timeout;
use upwell_core::RuntimeGenerationId;
use upwell_di::Component as _;

use super::fixture::{
    AliveCounts, Banner, GateMode, Greeter, Ledger, Spare, Tally, Watcher, alive_counts,
    build_recipe_app, config_dir_of, gate_parked, lock_test_guard, set_gate, set_tally_panics,
    vetoed_labels, write_recipe_config,
};
use super::relabel;
use crate::{App, AppRuntime};

/// The active instance of every recipe component, plus the generations they belong to.
struct Active {
    tally: Arc<Tally>,
    greeter: Arc<Greeter>,
    ledger: Arc<Ledger>,
    watcher: Arc<Watcher>,
    banner: Arc<Banner>,
    spare: Arc<Spare>,
    generation: RuntimeGenerationId,
    config_generation: u64,
}

impl Active {
    fn of(runtime: &AppRuntime) -> Self {
        let view = runtime.view();
        let root = view.root();

        Self {
            tally: root.get::<Tally>().expect("the tally resolves"),
            greeter: root.get::<Greeter>().expect("the greeter resolves"),
            ledger: root.get::<Ledger>().expect("the ledger resolves"),
            watcher: root.get::<Watcher>().expect("the watcher resolves"),
            banner: root.get::<Banner>().expect("the banner resolves"),
            spare: root.get::<Spare>().expect("the spare resolves"),
            generation: view.id(),
            config_generation: runtime.config_generation(),
        }
    }

    /// Asserts that `runtime` still serves exactly these instances, unchanged, and that
    /// no candidate instance outlived its attempt.
    fn assert_untouched(&self, runtime: &AppRuntime, outcome: &str) {
        let now = Self::of(runtime);

        assert_eq!(
            now.generation, self.generation,
            "{outcome}: generation moved"
        );
        assert_eq!(
            now.config_generation, self.config_generation,
            "{outcome}: config generation moved"
        );
        assert!(
            Arc::ptr_eq(&now.tally, &self.tally)
                && Arc::ptr_eq(&now.greeter, &self.greeter)
                && Arc::ptr_eq(&now.ledger, &self.ledger)
                && Arc::ptr_eq(&now.watcher, &self.watcher)
                && Arc::ptr_eq(&now.banner, &self.banner)
                && Arc::ptr_eq(&now.spare, &self.spare),
            "{outcome}: an active instance was swapped"
        );
        assert_eq!(
            self.banner.label(),
            "one",
            "{outcome}: the prepared hot update was applied"
        );
        assert!(
            Arc::ptr_eq(&self.watcher.tally.snapshot(), &self.tally),
            "{outcome}: the prepared live rebind was applied"
        );
        assert_eq!(
            alive_counts(),
            ONE_EACH,
            "{outcome}: a candidate instance outlived its attempt"
        );
    }
}

const ONE_EACH: AliveCounts = AliveCounts {
    tallies: 1,
    greeters: 1,
    ledgers: 1,
};

async fn build_matrix_app() -> (tempfile::TempDir, App<()>) {
    let (dir, app) = build_recipe_app("one").await;

    assert_eq!(alive_counts(), ONE_EACH, "the app builds one of each");

    (dir, app)
}

/// Asserts the gate ran for the attempt, so every strategy had prepared before it.
fn assert_gate_reached() {
    assert_eq!(
        vetoed_labels().last().map(String::as_str),
        Some("one"),
        "the gate ran after candidate construction and preparation, against the retained \
         banner"
    );
}

#[tokio::test]
async fn one_transition_applies_every_strategy_together() {
    let _guard = lock_test_guard().await;
    let (dir, app) = build_matrix_app().await;
    let runtime = app.runtime();
    let before = Active::of(runtime);

    relabel(&dir, runtime, "two").await;

    let after = Active::of(runtime);

    assert_eq!(after.generation.get(), before.generation.get() + 1);
    assert_eq!(after.tally.label, "two", "the tally reconstructed");
    assert_eq!(
        after.greeter.origin, "ordinary",
        "the declining transition handler fell back to reconstruction"
    );
    assert!(Arc::ptr_eq(&after.greeter.tally, &after.tally));
    assert_eq!(
        after.ledger.entries, 1,
        "the replacement carried the ledger"
    );
    assert!(Arc::ptr_eq(&after.ledger.tally, &after.tally));
    assert!(
        Arc::ptr_eq(&after.watcher, &before.watcher)
            && Arc::ptr_eq(&after.watcher.tally.snapshot(), &after.tally),
        "the watcher kept its instance and was rebound to the new tally"
    );
    assert!(
        Arc::ptr_eq(&after.banner, &before.banner) && after.banner.label() == "two",
        "the banner kept its instance and hot-updated"
    );
    assert!(
        Arc::ptr_eq(&after.spare, &before.spare),
        "the spare retained"
    );

    drop(before);

    assert_eq!(
        alive_counts(),
        ONE_EACH,
        "the displaced instances dropped once released"
    );
}

#[tokio::test]
async fn a_commit_displaces_exactly_the_constructed_strategies() {
    let _guard = lock_test_guard().await;
    let (dir, app) = build_matrix_app().await;

    write_recipe_config(&config_dir_of(&dir), "two");

    let report = app
        .runtime()
        .reload_config()
        .await
        .expect("the reload commits");
    let mut displaced = report.displaced().to_vec();

    displaced.sort_unstable();

    assert_eq!(
        displaced,
        [Greeter::ID, Ledger::ID, Tally::ID],
        "reconstructed, handler-built and replaced instances are displaced; retained, \
         rebound and hot-updated ones are not"
    );
}

#[tokio::test]
async fn a_rejection_after_every_strategy_prepared_applies_nothing() {
    let _guard = lock_test_guard().await;
    let (dir, app) = build_matrix_app().await;
    let runtime = app.runtime();
    let before = Active::of(runtime);

    set_gate(GateMode::Veto);
    write_recipe_config(&config_dir_of(&dir), "two");

    runtime
        .reload_config()
        .await
        .expect_err("the gate rejects the prepared transition");

    assert_gate_reached();
    before.assert_untouched(runtime, "after the rejection");
}

#[tokio::test]
async fn cancelling_after_every_strategy_prepared_applies_nothing() {
    let _guard = lock_test_guard().await;
    let (dir, app) = build_matrix_app().await;
    let runtime = app.runtime();
    let before = Active::of(runtime);
    let reloading = runtime.clone();

    set_gate(GateMode::Park);
    write_recipe_config(&config_dir_of(&dir), "two");

    let task = tokio::spawn(async move { reloading.reload_config().await });

    timeout(Duration::from_secs(5), gate_parked())
        .await
        .expect("the reload parks at the gate after every strategy prepared");
    task.abort();

    let cancelled = task
        .await
        .expect_err("the aborted reload does not complete");

    assert!(
        cancelled.is_cancelled(),
        "the reload was cancelled, not failed"
    );
    assert_gate_reached();
    before.assert_untouched(runtime, "after the cancellation");

    set_gate(GateMode::Accept);
    relabel(&dir, runtime, "three").await;

    let after = Active::of(runtime);

    assert_eq!(
        after.generation.get(),
        before.generation.get() + 1,
        "the cancelled attempt consumed no generation and released both leases"
    );
    assert_eq!(after.tally.label, "three");
    assert_eq!(after.banner.label(), "three");
    assert_eq!(
        after.ledger.entries, 1,
        "the cancelled replacement never became the previous instance"
    );
}

#[tokio::test]
async fn a_panicking_reconstruction_applies_nothing_and_later_reloads_commit() {
    let _guard = lock_test_guard().await;
    let (dir, app) = build_matrix_app().await;
    let runtime = app.runtime();
    let before = Active::of(runtime);

    set_tally_panics(true);
    write_recipe_config(&config_dir_of(&dir), "two");

    runtime
        .reload_config()
        .await
        .expect_err("the panicking factory rejects the transition");

    assert!(
        vetoed_labels().is_empty(),
        "construction failed before any later step ran"
    );
    before.assert_untouched(runtime, "after the panic");

    set_tally_panics(false);
    relabel(&dir, runtime, "three").await;

    assert_eq!(Active::of(runtime).tally.label, "three");
}
