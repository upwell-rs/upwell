//! The invocation context each lifecycle and transition hook runs in, and the generation it
//! addresses.

use upwell_di::context::{Invocation, InvocationContext};

use crate::app::{build, collect_contexts, collected_contexts, freeze, lock, write_mode};

fn hook_kinds(contexts: &[InvocationContext]) -> Vec<&'static str> {
    contexts
        .iter()
        .map(|context| match context.invocation() {
            Invocation::Hook { kind } => *kind,
            other => panic!("a hook runs in a hook context, not {other:?}"),
        })
        .collect()
}

#[tokio::test]
async fn each_reload_hook_runs_in_a_context_on_the_generation_it_addresses() {
    let _guard = lock().await;
    let (_dir, config_dir, app) = build("default").await;
    let runtime = app.runtime();
    let base = runtime.root();

    freeze(Some("change-freeze"));
    write_mode(&config_dir, "custom");
    collect_contexts();
    runtime
        .reload_config()
        .await
        .expect_err("the change freeze rejects the switch");

    let rejected = collected_contexts();

    assert_eq!(
        hook_kinds(&rejected),
        ["reload", "reload", "reload", "transition_aborted"]
    );
    assert!(
        rejected[0].resolves_root(&base),
        "proposal runs on the base"
    );
    assert!(
        !rejected[1].resolves_root(&base) && !rejected[2].resolves_root(&base),
        "config-reload and decision hooks run on the candidate"
    );
    assert!(
        rejected[3].resolves_root(&base),
        "the abort observes the base"
    );

    drop(rejected);
    freeze(None);
    collect_contexts();
    runtime
        .reload_config()
        .await
        .expect("the unfrozen switch commits");

    let committed = collected_contexts();
    let published = runtime.root();

    assert_eq!(
        hook_kinds(&committed),
        [
            "reload",
            "reload",
            "reload",
            "component_retired",
            "component_retired",
            "transition_committed"
        ]
    );
    assert!(committed[0].resolves_root(&base));
    assert!(committed[1].resolves_root(&published));
    assert!(committed[2].resolves_root(&published));
    assert!(committed[3].resolves_root(&base));
    assert!(committed[4].resolves_root(&base));
    assert!(committed[5].resolves_root(&published));
    assert!(
        committed
            .iter()
            .all(|context| context.scope_key().is_none()),
        "no hook runs in a scope"
    );
    assert!(
        InvocationContext::capture().is_err(),
        "a reload leaves its caller outside any context"
    );
}

#[tokio::test]
async fn startup_and_shutdown_hooks_run_in_a_context_on_the_current_generation() {
    let _guard = lock().await;
    let (_dir, _config_dir, app) = build("default").await;
    let root = app.runtime().root();
    let shutdown = app.shutdown_handle();

    collect_contexts();
    shutdown.shutdown();
    app.run().await.expect("the app runs until shutdown");

    let contexts = collected_contexts();

    assert_eq!(hook_kinds(&contexts), ["startup", "shutdown"]);
    assert!(contexts.iter().all(|context| context.resolves_root(&root)));
    assert!(contexts.iter().all(|context| context.scope_key().is_none()));
}
