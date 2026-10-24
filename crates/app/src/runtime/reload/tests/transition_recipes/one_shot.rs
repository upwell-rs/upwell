//! #293 acceptance: a transition handler selects a one-shot factory for one transition,
//! later transitions fall back to the declared strategy, undeclared dependencies are
//! rejected, and failures or cancellation leave the previous generation intact.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::timeout;

use super::fixture::{
    Greeter, OneShotMode, build_recipe_app, config_dir_of, gate_parked, lock_test_guard,
    set_one_shot, write_recipe_config,
};
use super::relabel;
use crate::{TransitionFailure, TransitionPhase};

fn greeter(app: &crate::App<()>) -> Arc<Greeter> {
    app.runtime()
        .root()
        .get::<Greeter>()
        .expect("the greeter resolves from the current root")
}

#[tokio::test]
async fn a_selected_one_shot_factory_builds_one_transition_then_the_declared_strategy_resumes() {
    let _guard = lock_test_guard().await;
    let (dir, app) = build_recipe_app("one").await;

    assert_eq!(greeter(&app).origin, "ordinary");

    set_one_shot(OneShotMode::Fresh);
    relabel(&dir, app.runtime(), "two").await;

    let one_shot = greeter(&app);

    assert_eq!(
        one_shot.origin, "one-shot:fixed-dependent",
        "the handler saw the planned change and its factory captured it"
    );
    assert_eq!(
        one_shot.tally.label, "two",
        "the one-shot sees candidate dependencies"
    );

    relabel(&dir, app.runtime(), "three").await;

    let declared = greeter(&app);

    assert_eq!(
        declared.origin, "ordinary",
        "a later transition falls back to the declared strategy"
    );
    assert_eq!(declared.tally.label, "three");
}

#[tokio::test]
async fn a_one_shot_factory_may_read_the_previous_instance() {
    let _guard = lock_test_guard().await;
    let (dir, app) = build_recipe_app("one").await;
    let previous = greeter(&app);

    set_one_shot(OneShotMode::Carry);
    relabel(&dir, app.runtime(), "two").await;

    let replacement = greeter(&app);

    assert!(!Arc::ptr_eq(&previous, &replacement));
    assert_eq!(replacement.origin, "one-shot after ordinary");
}

#[tokio::test]
async fn a_one_shot_factory_resolving_an_undeclared_dependency_is_rejected() {
    let _guard = lock_test_guard().await;
    let (dir, app) = build_recipe_app("one").await;
    let previous = greeter(&app);
    let generation = app.runtime().generation();

    set_one_shot(OneShotMode::Undeclared);
    write_recipe_config(&config_dir_of(&dir), "two");

    let error = app
        .runtime()
        .reload_config()
        .await
        .expect_err("the undeclared dependency aborts the reload");
    let (report, error) = super::super::rejected(error);
    let message = error.to_string();

    assert_eq!(
        report.outcome(),
        &crate::TransitionOutcome::Rejected {
            phase: TransitionPhase::Construct,
            failure: TransitionFailure::ConstructionFailed,
        }
    );
    assert!(
        matches!(
            error,
            crate::Error::Di(upwell_di::Error::UndeclaredTransitionDependency {
                role: upwell_di::ComponentFactoryRole::Transition,
                ..
            })
        ),
        "unexpected error: {message}"
    );
    assert!(
        message.contains("Spare") && message.contains("declare it on the ordinary factory"),
        "the diagnostic names the dependency and the fix: {message}"
    );
    assert_eq!(app.runtime().generation(), generation);
    assert!(Arc::ptr_eq(&previous, &greeter(&app)));
}

#[tokio::test]
async fn failing_or_panicking_one_shot_factories_leave_the_previous_generation_active() {
    let _guard = lock_test_guard().await;

    for (mode, failure) in [
        (OneShotMode::Fail, TransitionFailure::ConstructionFailed),
        (OneShotMode::Panic, TransitionFailure::Panicked),
    ] {
        let (dir, app) = build_recipe_app("one").await;
        let previous = greeter(&app);
        let generation = app.runtime().generation();

        set_one_shot(mode);
        write_recipe_config(&config_dir_of(&dir), "two");

        let error = app
            .runtime()
            .reload_config()
            .await
            .expect_err("the one-shot factory aborts the reload");
        let (report, _) = super::super::rejected(error);

        super::super::assert_rejected(&report, TransitionPhase::Construct, failure);
        assert_eq!(app.runtime().generation(), generation, "{mode:?}");
        assert!(Arc::ptr_eq(&previous, &greeter(&app)), "{mode:?}");
        assert_eq!(greeter(&app).tally.label, "one", "{mode:?}");
    }
}

#[tokio::test]
async fn cancelling_a_one_shot_construction_leaves_no_partial_state() {
    let _guard = lock_test_guard().await;
    let (dir, app) = build_recipe_app("one").await;
    let previous = greeter(&app);
    let generation = app.runtime().generation();
    let config_generation = app.runtime().config_generation();
    let reloading = app.runtime().clone();

    set_one_shot(OneShotMode::Pending);
    write_recipe_config(&config_dir_of(&dir), "two");

    let task = tokio::spawn(async move { reloading.reload_config().await });

    timeout(Duration::from_secs(5), gate_parked())
        .await
        .expect("the reload parks in the pending one-shot factory");
    task.abort();

    let cancelled = task
        .await
        .expect_err("the aborted reload does not complete");

    assert!(
        cancelled.is_cancelled(),
        "the reload was cancelled, not failed"
    );
    assert_eq!(app.runtime().generation(), generation);
    assert_eq!(app.runtime().config_generation(), config_generation);
    assert!(Arc::ptr_eq(&previous, &greeter(&app)));

    let report = app
        .runtime()
        .reload_config()
        .await
        .expect("the leases were released, so the next reload commits");

    assert!(report.is_committed());
    assert_eq!(
        greeter(&app).origin,
        "ordinary",
        "the cancelled one-shot was dropped with its attempt"
    );
}
