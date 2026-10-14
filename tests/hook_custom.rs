//! Proof that a hook kind defined *outside* the framework works end to end: this test crate
//! declares its own `Startup` kind, a component subscribes with `#[hook(Startup)]`, and the
//! test fires it through the injectable `HookManager` — the path any external crate uses.
//! Also exercises the O(1) `has::<K>()` listener check and the macro-declared
//! `observe`/`always` participant/observer split.
#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};

use upwell::config::Toml;
use upwell::{App, AppError, ConfigManager, HookKind, component, methods};

/// A user-defined lifecycle kind — no inputs, no output.
struct Startup;

impl HookKind for Startup {
    type Output = ();
    type Cx = ();

    const NAME: &'static str = "startup";
}

/// A kind nobody listens to, to prove `has` is false for it.
struct Unused;

impl HookKind for Unused {
    type Output = ();
    type Cx = ();

    const NAME: &'static str = "unused";
}

/// A participant-and-observer kind whose participants vote; a `false` vote ends the event.
struct Vote;

impl HookKind for Vote {
    type Output = bool;
    type Cx = ();

    const NAME: &'static str = "vote";

    fn ends_event(outcome: &upwell::HookResult<bool>) -> bool {
        !matches!(outcome, Ok(true))
    }
}

/// Declares one targeted participant, one targeted observer, and one `always` observer.
#[component]
struct Voter {
    #[default]
    observed: AtomicUsize,
}

#[methods]
impl Voter {
    #[hook(Vote)]
    async fn vote(&self) -> bool {
        self.observed.load(Ordering::SeqCst) == 0
    }

    #[hook(Vote, observe)]
    async fn audit(&self) -> Result<(), AppError> {
        self.observed.fetch_add(1, Ordering::SeqCst);

        Ok(())
    }

    #[hook(Vote, observe, always)]
    async fn audit_every_vote(&self) {
        self.observed.fetch_add(1, Ordering::SeqCst);
    }
}

/// Subscribes to the custom `Startup` kind. The hook takes `&self` and no inputs.
#[component]
struct Boot {
    #[default]
    started: AtomicUsize,
}

impl Boot {
    fn started(&self) -> usize {
        self.started.load(Ordering::SeqCst)
    }
}

#[methods]
impl Boot {
    #[hook(Startup)]
    async fn on_start(&self) -> Result<(), AppError> {
        self.started.fetch_add(1, Ordering::SeqCst);

        Ok(())
    }
}

#[tokio::test]
async fn external_hook_kind_fires_through_the_manager() {
    let daemon = App::<()>::builder("hook-custom-test")
        .config_source(ConfigManager::<Toml>::empty())
        .auto_discover()
        .build()
        .await
        .expect("daemon builds");

    let boot = daemon.container().get::<Boot>().expect("Boot built");
    let hooks = daemon.hook_manager();

    assert!(hooks.has::<Startup>(), "Startup has a listener");
    assert!(
        !hooks.has::<Unused>(),
        "Unused has no listeners (O(1) miss)"
    );
    assert_eq!(boot.started(), 0, "not started yet");

    let outcomes = hooks.fire::<Startup>(&(), |_| true).await.participants;

    assert_eq!(outcomes.len(), 1, "the one Startup hook ran");
    assert!(outcomes[0].1.is_ok(), "the hook succeeded");
    assert_eq!(boot.started(), 1, "the component observed the fire");

    // Firing a kind with no listeners is a no-op that does no work.
    let none = hooks.fire::<Unused>(&(), |_| true).await;
    assert!(
        none.participants.is_empty() && none.observers.is_empty(),
        "no listeners -> empty, silently dropped"
    );
}

#[tokio::test]
async fn macro_declared_roles_and_routing_drive_the_fire() {
    let daemon = App::<()>::builder("hook-roles-test")
        .config_source(ConfigManager::<Toml>::empty())
        .auto_discover()
        .build()
        .await
        .expect("daemon builds");
    let hooks = daemon.hook_manager();

    let unselected = hooks.fire::<Vote>(&(), |_| false).await;

    assert!(
        unselected.participants.is_empty(),
        "a targeted participant needs the site's selection"
    );
    assert_eq!(
        unselected.observers.len(),
        1,
        "only the `always` observer bypasses the selection"
    );

    let accepted = hooks.fire::<Vote>(&(), |_| true).await;

    assert!(
        accepted.ended,
        "the participant votes no after an observer ran"
    );
    assert!(matches!(accepted.participants.as_slice(), [(_, Ok(false))]));
    assert!(
        accepted.observers.is_empty(),
        "observers never run for an event a participant ended"
    );
}
