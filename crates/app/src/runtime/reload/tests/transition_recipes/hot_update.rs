//! #294 acceptance: an opted-in component updates in place only after the transition
//! commits, and a failed or rejected transition never runs the commit action.

use std::sync::Arc;

use super::fixture::{
    Banner, GateMode, HotUpdateMode, banner_constructions, build_recipe_app, config_dir_of,
    lock_test_guard, set_gate, set_hot_update, vetoed_labels, write_recipe_config,
};
use super::relabel;
use crate::{TransitionFailure, TransitionPhase};
use upwell_di::Component as _;

fn banner(app: &crate::App<()>) -> Arc<Banner> {
    app.runtime()
        .root()
        .get::<Banner>()
        .expect("the banner resolves from the current root")
}

#[tokio::test]
async fn a_hot_update_applies_a_config_change_in_place_after_commit() {
    let _guard = lock_test_guard().await;
    let (dir, app) = build_recipe_app("one").await;
    let original = banner(&app);

    write_recipe_config(&config_dir_of(&dir), "two");

    let report = app
        .runtime()
        .reload_config()
        .await
        .expect("the reload commits");

    assert!(report.is_committed());
    assert!(
        Arc::ptr_eq(&original, &banner(&app)),
        "the instance is kept"
    );
    assert_eq!(original.label(), "two", "the update applied at commit");
    assert_eq!(
        vetoed_labels(),
        ["one"],
        "the candidate hooks ran before the update applied"
    );
    assert_eq!(
        banner_constructions(),
        1,
        "the banner was never reconstructed"
    );
    assert!(
        !report.displaced().contains(&Banner::ID),
        "a hot-updated component is not displaced"
    );

    relabel(&dir, app.runtime(), "three").await;

    assert!(Arc::ptr_eq(&original, &banner(&app)));
    assert_eq!(original.label(), "three");
}

#[tokio::test]
async fn a_rejected_transition_never_runs_the_prepared_update() {
    let _guard = lock_test_guard().await;
    let (dir, app) = build_recipe_app("one").await;
    let original = banner(&app);
    let generation = app.runtime().generation();

    set_gate(GateMode::Veto);
    write_recipe_config(&config_dir_of(&dir), "two");

    let error = app
        .runtime()
        .reload_config()
        .await
        .expect_err("the hook vetoes after the update was prepared");
    let (report, _) = super::super::rejected(error);

    super::super::assert_rejected(
        &report,
        TransitionPhase::Hooks,
        TransitionFailure::HookRejected {
            component: Banner::ID,
        },
    );
    assert_eq!(app.runtime().generation(), generation);
    assert!(Arc::ptr_eq(&original, &banner(&app)));
    assert_eq!(original.label(), "one", "the prepared update was dropped");
}

#[tokio::test]
async fn a_panicking_apply_action_is_contained_and_the_publication_completes() {
    let _guard = lock_test_guard().await;
    let (dir, app) = build_recipe_app("one").await;
    let original = banner(&app);
    let generation = app.runtime().generation();
    let config_generation = app.runtime().config_generation();

    set_hot_update(HotUpdateMode::PanicOnApply);
    relabel(&dir, app.runtime(), "two").await;

    assert_eq!(app.runtime().generation().get(), generation.get() + 1);
    assert_eq!(
        app.runtime().config_generation(),
        config_generation + 1,
        "the config slots still committed after the contained panic"
    );
    assert!(Arc::ptr_eq(&original, &banner(&app)));
    assert_eq!(
        original.label(),
        "one",
        "the panicking action applied nothing"
    );

    set_hot_update(HotUpdateMode::Apply);
    relabel(&dir, app.runtime(), "three").await;

    assert_eq!(
        original.label(),
        "three",
        "later reloads still apply updates"
    );
}

#[tokio::test]
async fn failing_or_panicking_preparations_leave_the_instance_untouched() {
    let _guard = lock_test_guard().await;

    for (mode, failure) in [
        (HotUpdateMode::Fail, TransitionFailure::ConstructionFailed),
        (HotUpdateMode::Panic, TransitionFailure::Panicked),
    ] {
        let (dir, app) = build_recipe_app("one").await;
        let original = banner(&app);
        let generation = app.runtime().generation();

        set_hot_update(mode);
        write_recipe_config(&config_dir_of(&dir), "two");

        let error = app
            .runtime()
            .reload_config()
            .await
            .expect_err("the preparation aborts the reload");
        let (report, _) = super::super::rejected(error);

        super::super::assert_rejected(&report, TransitionPhase::Construct, failure);
        assert_eq!(app.runtime().generation(), generation, "{mode:?}");
        assert!(Arc::ptr_eq(&original, &banner(&app)), "{mode:?}");
        assert_eq!(original.label(), "one", "{mode:?}");
    }
}
