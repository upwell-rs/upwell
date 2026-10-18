//! Static state-aware replacement through the transactional reload: a component that
//! declares a replacement factory carries state from its previous instance into the
//! replacement, resolves its dependencies from the candidate generation only, and a
//! failing or panicking replacement leaves the previous instance active and usable.

mod fixture;

use std::sync::Arc;

use upwell_di::{Component as _, NodeAction};

use fixture::{
    Counter, Replacement, Tally, build_counter_app, config_dir_of, lock_test_guard,
    set_replacement, write_counter_config,
};

use crate::{TransitionFailure, TransitionPhase};

#[tokio::test]
async fn replacement_carries_state_from_the_previous_instance_across_a_config_change() {
    let _guard = lock_test_guard().await;
    let (dir, app) = build_counter_app("one").await;
    let previous = app
        .runtime()
        .root()
        .get::<Counter>()
        .expect("counter builds");

    previous.hit();
    previous.hit();
    previous.hit();
    write_counter_config(&config_dir_of(&dir), "two");

    let report = app
        .runtime()
        .reload_config()
        .await
        .expect("the reload commits");
    let replacement = app
        .runtime()
        .root()
        .get::<Counter>()
        .expect("the replacement is active");

    assert!(report.is_committed());
    assert!(
        report
            .components()
            .iter()
            .any(|component| component.component == Counter::ID
                && component.action == NodeAction::Replace),
        "the counter is replaced, not retained"
    );
    assert!(
        !Arc::ptr_eq(&previous, &replacement),
        "a new instance is active"
    );
    assert_eq!(replacement.hits(), 3, "the hit count is carried forward");
    assert_eq!(
        replacement.tally.label, "two",
        "the replacement sees the new config"
    );
    assert_eq!(previous.hits(), 3, "the previous instance is never mutated");

    replacement.hit();

    assert_eq!(previous.hits(), 3, "the instances share no mutable state");
}

#[tokio::test]
async fn replacement_resolves_dependencies_from_the_candidate_generation() {
    let _guard = lock_test_guard().await;
    let (dir, app) = build_counter_app("one").await;
    let active_tally = app.runtime().root().get::<Tally>().expect("tally builds");

    write_counter_config(&config_dir_of(&dir), "two");
    app.runtime()
        .reload_config()
        .await
        .expect("the reload commits");

    let root = app.runtime().root();
    let candidate_tally = root.get::<Tally>().expect("the candidate tally is active");
    let replacement = root.get::<Counter>().expect("the replacement is active");

    assert!(
        Arc::ptr_eq(&replacement.tally, &candidate_tally),
        "the replacement injects the candidate tally"
    );
    assert!(
        !Arc::ptr_eq(&replacement.tally, &active_tally),
        "the stale active tally is unreachable"
    );
    assert_eq!(active_tally.label, "one");
}

#[tokio::test]
async fn failing_or_panicking_replacements_leave_the_previous_instance_active_and_usable() {
    let _guard = lock_test_guard().await;

    for (behavior, failure) in [
        (Replacement::Fail, TransitionFailure::ConstructionFailed),
        (Replacement::Panic, TransitionFailure::Panicked),
    ] {
        let (dir, app) = build_counter_app("one").await;
        let previous = app
            .runtime()
            .root()
            .get::<Counter>()
            .expect("counter builds");
        let generation = app.runtime().generation();

        previous.hit();
        set_replacement(behavior);
        write_counter_config(&config_dir_of(&dir), "two");

        let error = app
            .runtime()
            .reload_config()
            .await
            .expect_err("the replacement aborts the reload");
        let (report, _) = super::rejected(error);
        let active = app
            .runtime()
            .root()
            .get::<Counter>()
            .expect("counter stays");

        super::assert_rejected(&report, TransitionPhase::Construct, failure);
        assert_eq!(app.runtime().generation(), generation, "{behavior:?}");
        assert!(
            Arc::ptr_eq(&previous, &active),
            "{behavior:?}: still active"
        );
        assert_eq!(active.tally.label, "one", "{behavior:?}: old config stays");
        assert_eq!(active.hit(), 2, "{behavior:?}: the instance stays usable");
    }
}
