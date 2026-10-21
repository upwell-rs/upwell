//! Each recipe attribute drives its strategy across a reload, in both directions.

use std::sync::Arc;

use crate::app::{Banner, Greeter, Ledger, build, write_config};

#[tokio::test]
async fn recipe_attributes_drive_their_strategies_in_both_directions() {
    let (_dir, config_dir, app) = build("calm", "first").await;
    let runtime = app.runtime();
    let banner = runtime.root().get::<Banner>().expect("the banner resolves");
    let mut ledgers = Vec::new();
    let mut greeters = Vec::new();

    for (mode, label) in [("loud", "second"), ("calm", "third")] {
        ledgers.push(runtime.root().get::<Ledger>().expect("the ledger resolves"));
        greeters.push(
            runtime
                .root()
                .get::<Greeter>()
                .expect("the greeter resolves"),
        );

        write_config(&config_dir, mode, label);
        runtime.reload_config().await.expect("the reload commits");

        let ledger = runtime.root().get::<Ledger>().expect("the ledger resolves");
        let relabeled = runtime.root().get::<Banner>().expect("the banner resolves");

        assert_eq!(
            ledger.mode.name(),
            mode,
            "the replacement sees the new provider"
        );
        assert!(
            Arc::ptr_eq(&banner, &relabeled),
            "the hot update keeps the banner"
        );
        assert_eq!(banner.label(), label, "the hot update relabels in place");
    }

    let ledger = runtime.root().get::<Ledger>().expect("the ledger resolves");
    let greeter = runtime
        .root()
        .get::<Greeter>()
        .expect("the greeter resolves");
    let entries: Vec<_> = ledgers.iter().map(|ledger| ledger.entries).collect();

    assert_eq!(
        entries,
        [0, 1],
        "each replacement carried the previous count"
    );
    assert_eq!(ledger.entries, 2);
    assert_eq!(greeters[0].origin, "ordinary");
    assert_eq!(
        greeter.origin, "one-shot:fixed-dependent after one-shot:fixed-dependent after ordinary",
        "each transition picked a one-shot factory that read the previous greeter"
    );
}
