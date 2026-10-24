//! #302 lifecycle hardening over the Authenticator provider switch: every injected failure
//! leaves the complete previous generation usable, repeated and overlapping switches stay
//! consistent, invalid candidates fail with actionable diagnostics, and already open scopes
//! keep the generation they pinned.

mod diagnostics;
mod failures;
mod faults;
mod repeated;
mod scopes;

use std::sync::Arc;

use tempfile::TempDir;

use super::authenticator_scheme;
use super::fixture::{
    FallbackAuthenticator, PRIMARY_COMPONENT, PrimaryAuthenticator, auth_builder, auth_config_dir,
    config_dir_of, fail_fallback_factory, panic_fact_extraction, panic_fallback_factory,
    write_auth_config,
};
use super::gate::AuthGate;
use super::gate_hooks::{AUDIT_COMPONENT, AuditLog, Controls, HOOKED_GATE_COMPONENT, set_controls};
use crate::{App, RuntimeView};
use faults::{ConfigHookFault, ORPHAN_COMPONENT, SENTINEL_COMPONENT, set_config_hook_fault};

/// The primary-mode app with hooked gate, unaffected audit log, config-hook sentinel and
/// orphan consumer, and every fault control reset.
async fn hardening_app() -> (TempDir, App<()>) {
    let dir = auth_config_dir("primary");

    reset_faults();

    let app = auth_builder::<()>(
        &config_dir_of(&dir),
        &PRIMARY_COMPONENT,
        &HOOKED_GATE_COMPONENT,
        &[&AUDIT_COMPONENT, &SENTINEL_COMPONENT, &ORPHAN_COMPONENT],
    )
    .build()
    .await
    .expect("the hardening app builds");

    (dir, app)
}

fn reset_faults() {
    set_controls(Controls::default());
    set_config_hook_fault(ConfigHookFault::Accept);
    fail_fallback_factory(false);
    panic_fallback_factory(false);
    panic_fact_extraction(false);
}

/// Everything the active generation consists of before an attempt.
struct Baseline {
    view: RuntimeView,
    generation: u64,
    config_generation: u64,
    primary: Arc<PrimaryAuthenticator>,
    gate: Arc<AuthGate>,
    audit: Arc<AuditLog>,
}

impl Baseline {
    fn capture(app: &App<()>) -> Self {
        let runtime = app.runtime();
        let root = runtime.root();

        Self {
            view: runtime.view(),
            generation: runtime.generation().get(),
            config_generation: runtime.config_generation(),
            primary: root
                .get::<PrimaryAuthenticator>()
                .expect("primary resolves"),
            gate: root.get::<AuthGate>().expect("gate resolves"),
            audit: root.get::<AuditLog>().expect("audit log resolves"),
        }
    }

    /// Asserts that the generation captured here is still the complete active one: the
    /// same generation and config generation, graph, and instances, and that it still
    /// resolves the primary provider through the trait mapping.
    async fn assert_intact(&self, app: &App<()>, case: &str) {
        let runtime = app.runtime();
        let view = runtime.view();
        let root = runtime.root();

        assert_eq!(
            runtime.generation().get(),
            self.generation,
            "{case}: generation"
        );
        assert_eq!(
            runtime.config_generation(),
            self.config_generation,
            "{case}: config generation"
        );
        assert!(
            Arc::ptr_eq(view.effective_graph(), self.view.effective_graph()),
            "{case}: the active graph is unchanged"
        );
        assert!(
            Arc::ptr_eq(
                &root
                    .get::<PrimaryAuthenticator>()
                    .expect("primary resolves"),
                &self.primary
            ),
            "{case}: the primary instance is unchanged"
        );
        assert!(
            Arc::ptr_eq(&root.get::<AuthGate>().expect("gate resolves"), &self.gate),
            "{case}: the gate instance is unchanged"
        );
        assert!(
            Arc::ptr_eq(
                &root.get::<AuditLog>().expect("audit resolves"),
                &self.audit
            ),
            "{case}: the audit instance is unchanged"
        );
        assert!(
            root.get::<FallbackAuthenticator>().is_none(),
            "{case}: no candidate instance leaked into the active root"
        );
        assert_eq!(
            self.gate.scheme, "primary",
            "{case}: the gate kept its provider"
        );
        assert_eq!(
            authenticator_scheme(&root).await,
            "primary",
            "{case}: the trait still resolves the primary"
        );
        assert!(
            runtime.displaced_generations().is_empty(),
            "{case}: nothing was displaced"
        );
    }
}

/// Switches to the fallback with every fault cleared and asserts that it commits.
async fn assert_recovers(dir: &TempDir, app: &App<()>, case: &str) {
    reset_faults();
    write_auth_config(&config_dir_of(dir), "fallback");

    let report = app
        .runtime()
        .reload_config()
        .await
        .unwrap_or_else(|error| panic!("{case}: the later valid switch commits: {error}"));

    assert!(report.is_committed(), "{case}: the later switch commits");
    assert_eq!(
        authenticator_scheme(&app.runtime().root()).await,
        "fallback",
        "{case}: the later switch selects the fallback"
    );
}
