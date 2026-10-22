//! Repeated, no-op and overlapping switches: every committed switch publishes exactly one
//! generation, unaffected components stay retained across all of them, every pinned
//! generation keeps its own provider, and a burst of concurrent reloads commits once.

use futures::future::join_all;

use super::super::fixture::lock_test_guard;
use super::*;
use crate::TransitionOutcome;

#[tokio::test]
async fn alternating_switches_stay_consistent_and_pinned_generations_keep_their_provider() {
    let _guard = lock_test_guard().await;
    let (dir, app) = hardening_app().await;
    let runtime = app.runtime();
    let audit = runtime.root().get::<AuditLog>().expect("audit resolves");
    let mut pinned = vec![(runtime.view(), "primary")];

    for round in 1..=6_u64 {
        let mode = if round % 2 == 1 {
            "fallback"
        } else {
            "primary"
        };

        write_auth_config(&config_dir_of(&dir), mode);

        let report = runtime
            .reload_config()
            .await
            .unwrap_or_else(|error| panic!("round {round}: the switch commits: {error}"));
        let noop = runtime
            .reload_config()
            .await
            .unwrap_or_else(|error| panic!("round {round}: the repeat is a no-op: {error}"));
        let root = runtime.root();

        assert!(report.is_committed(), "round {round}: committed");
        assert_eq!(noop.outcome(), &TransitionOutcome::NoOp, "round {round}");
        assert_eq!(
            runtime.generation().get(),
            round,
            "round {round}: one generation"
        );
        assert_eq!(authenticator_scheme(&root).await, mode, "round {round}");
        assert_eq!(
            root.get::<AuthGate>().expect("gate resolves").scheme,
            mode,
            "round {round}: the fixed gate was reconstructed onto the new provider"
        );
        assert!(
            Arc::ptr_eq(&root.get::<AuditLog>().expect("audit resolves"), &audit),
            "round {round}: the unaffected audit log is retained"
        );

        pinned.push((runtime.view(), mode));
    }

    for (view, mode) in &pinned {
        assert_eq!(
            authenticator_scheme(view.root()).await,
            *mode,
            "generation {} keeps its own provider",
            view.id().get()
        );
    }
}

#[tokio::test]
async fn a_burst_of_concurrent_reloads_commits_the_switch_exactly_once() {
    let _guard = lock_test_guard().await;
    let (dir, app) = hardening_app().await;
    let runtime = app.runtime();

    write_auth_config(&config_dir_of(&dir), "fallback");

    let reports = join_all((0..8).map(|_| runtime.reload_config())).await;
    let committed = reports
        .iter()
        .map(|report| report.as_ref().expect("every concurrent reload succeeds"))
        .filter(|report| report.is_committed())
        .count();
    let attempts = reports
        .iter()
        .flatten()
        .map(|report| report.attempt())
        .collect::<std::collections::BTreeSet<_>>();

    assert_eq!(
        committed, 1,
        "the serialized reloads commit the switch once"
    );
    assert_eq!(attempts.len(), 8, "every attempt has its own identity");
    assert_eq!(runtime.generation().get(), 1);
    assert_eq!(runtime.config_generation(), 1);
    assert_eq!(authenticator_scheme(&runtime.root()).await, "fallback");
}
