//! The #206 acceptance path: switching the file-backed `auth.mode` between the two
//! mutually exclusive values transactionally swaps the effective `Authenticator`
//! provider in both directions — condition-driven removal-by-absence plus
//! addition/reconstruction — while every pinned generation keeps resolving its own
//! provider through its own root.

mod fixture;

use std::sync::Arc;

use upwell_di::{Component as _, ScopeContainer};

use fixture::{
    Authenticator, FallbackAuthenticator, PrimaryAuthenticator, build_auth_app,
    clear_observed_modes, config_dir_of, observed_modes, write_auth_config,
};

/// Resolves the effective `Authenticator` provider through the trait mapping and
/// returns its scheme.
async fn authenticator_scheme(root: &Arc<ScopeContainer>) -> String {
    let authenticator = root
        .extract::<Arc<dyn Authenticator>>()
        .await
        .expect("the eligible Authenticator provider resolves from the root");

    authenticator.scheme().to_owned()
}

#[tokio::test]
async fn config_switch_swaps_the_effective_authenticator_provider_transactionally() {
    let (dir, app) = build_auth_app("primary").await;
    let runtime = app.runtime();

    let old_view = runtime.view();
    let old_generation = runtime.generation();
    let old_config_generation = app.config_reloader().generation();

    // The initial build resolves the primary through the trait mapping, and its
    // factory read the initial config value.
    assert_eq!(authenticator_scheme(old_view.root()).await, "primary");
    assert_eq!(
        old_view
            .root()
            .get::<PrimaryAuthenticator>()
            .expect("the primary component resolves from the initial root")
            .observed_mode,
        "primary"
    );
    assert!(
        old_view
            .effective_graph()
            .node(PrimaryAuthenticator::ID)
            .is_some(),
        "the primary is in the initial effective graph"
    );
    assert!(
        old_view
            .effective_graph()
            .node(FallbackAuthenticator::ID)
            .is_none(),
        "the fallback is absent from the initial effective graph"
    );
    assert_eq!(observed_modes().await, ["primary"]);

    clear_observed_modes().await;

    // Switch to the fallback and reload: the primary is removed by absence and the
    // fallback is reconstructed against the staged proposal.
    write_auth_config(&config_dir_of(&dir), "fallback");

    let report = runtime.reload_config().await.expect("reload succeeds");

    assert!(report.published, "a changed source publishes a generation");
    assert_eq!(report.changed.len(), 1, "only the auth binding changed");
    assert_eq!(report.changed[0].path, "auth");
    assert_eq!(report.runtime_generation, runtime.generation());
    assert_eq!(
        runtime.generation().get(),
        old_generation.get() + 1,
        "the runtime generation advances exactly once"
    );
    assert_eq!(
        app.config_reloader().generation(),
        old_config_generation + 1,
        "the config generation advances once, at the terminal commit"
    );

    // The new generation resolves the fallback, whose factory read the staged value.
    let fallback_view = runtime.view();
    assert_eq!(authenticator_scheme(fallback_view.root()).await, "fallback");
    assert_eq!(
        fallback_view
            .root()
            .get::<FallbackAuthenticator>()
            .expect("the fallback component resolves from the new root")
            .observed_mode,
        "fallback"
    );
    assert!(
        fallback_view
            .effective_graph()
            .node(FallbackAuthenticator::ID)
            .is_some(),
        "the fallback is in the new effective graph"
    );
    assert!(
        fallback_view
            .effective_graph()
            .node(PrimaryAuthenticator::ID)
            .is_none(),
        "the primary was removed from the effective graph"
    );
    assert_eq!(observed_modes().await, ["fallback"]);

    // The pinned old generation still represents the primary: its root, graph, and
    // provider mapping belong to that generation and are untouched by the
    // transaction. A legacy live `Cfg` handle shared with the active slots is
    // deliberately not asserted frozen — a pinned view does not freeze live cells.
    assert_eq!(old_view.id(), old_generation);
    assert_eq!(
        authenticator_scheme(old_view.root()).await,
        "primary",
        "the pinned old root still resolves its own provider"
    );
    assert!(
        old_view.root().get::<PrimaryAuthenticator>().is_some(),
        "the pinned old root still holds the primary component"
    );
    assert!(
        old_view
            .effective_graph()
            .node(PrimaryAuthenticator::ID)
            .is_some(),
        "the pinned old graph still contains the primary"
    );
    assert!(
        old_view
            .effective_graph()
            .node(FallbackAuthenticator::ID)
            .is_none(),
        "the pinned old graph still lacks the fallback"
    );

    clear_observed_modes().await;

    // Switch back to the primary: the reverse transition reconstructs it and removes
    // the fallback by absence.
    write_auth_config(&config_dir_of(&dir), "primary");

    let report = runtime
        .reload_config()
        .await
        .expect("the reverse reload succeeds");

    assert!(report.published, "the reverse switch also publishes");
    assert_eq!(
        report.runtime_generation,
        runtime.generation(),
        "the report names the generation it published"
    );
    assert_eq!(
        runtime.generation().get(),
        old_generation.get() + 2,
        "the runtime generation advances exactly once more"
    );
    assert_eq!(
        app.config_reloader().generation(),
        old_config_generation + 2,
        "the config generation advances once more"
    );

    // The rebuilt generation resolves the primary again, whose factory read the
    // staged value.
    let primary_view = runtime.view();
    assert_eq!(authenticator_scheme(primary_view.root()).await, "primary");
    assert_eq!(
        primary_view
            .root()
            .get::<PrimaryAuthenticator>()
            .expect("the primary component resolves from the rebuilt root")
            .observed_mode,
        "primary"
    );
    assert!(
        primary_view
            .effective_graph()
            .node(PrimaryAuthenticator::ID)
            .is_some(),
        "the primary is back in the effective graph"
    );
    assert!(
        primary_view
            .effective_graph()
            .node(FallbackAuthenticator::ID)
            .is_none(),
        "the fallback was removed from the effective graph"
    );
    assert_eq!(observed_modes().await, ["primary"]);

    // The pinned middle generation still represents the fallback after the second
    // swap: its exact generation identity, root, graph, and provider mapping belong
    // to that generation and are untouched by the second transaction.
    assert_eq!(
        fallback_view.id().get(),
        old_generation.get() + 1,
        "the pinned middle view is exactly the fallback generation"
    );
    assert_eq!(
        authenticator_scheme(fallback_view.root()).await,
        "fallback",
        "the pinned middle root still resolves its own provider"
    );
    assert_eq!(
        fallback_view
            .root()
            .get::<FallbackAuthenticator>()
            .expect("the pinned middle root still holds the fallback component")
            .observed_mode,
        "fallback"
    );
    assert!(
        fallback_view.root().get::<PrimaryAuthenticator>().is_none(),
        "the pinned middle root still lacks the primary component"
    );
    assert!(
        fallback_view
            .effective_graph()
            .node(FallbackAuthenticator::ID)
            .is_some(),
        "the pinned middle graph still contains the fallback"
    );
    assert!(
        fallback_view
            .effective_graph()
            .node(PrimaryAuthenticator::ID)
            .is_none(),
        "the pinned middle graph still lacks the primary"
    );
}
