//! Fixture for the Authenticator provider-switch acceptance test: a file-backed app
//! with two condition-gated `Authenticator` provider components whose availability
//! keys on opposite sides of one `auth.mode` config fact, so exactly one provider is
//! eligible at any time.
//!
//! The components are declared with explicit builder descriptors and unique ids — no
//! `#[component]` macros — so nothing registers into the link-time inventory and the
//! test cannot interfere with discovery-based tests. Each factory injects
//! `Cfg<AuthConfig>` and records the config mode its construction resolved, so the
//! test can prove which store each stage of the transaction read. A fixed
//! [`AuthGate`](super::gate::AuthGate) consumer injects the effective provider, so the
//! switch has an affected consumer to report. The fixture owns its statics alone and
//! shares nothing with the probe fixture, but its own tests share [`OBSERVED_MODES`], so
//! [`TEST_GUARD`] serializes them under the default `cargo test` parallelism.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;
use tempfile::TempDir;
use upwell_config::{Cfg, ConditionFacts, ConfigManager, ConfigProperties, ResolverChain, Toml};
use upwell_core::{
    ConditionDescriptor, ConditionScalar, ConditionScalarKind, ConfigFactDescriptor, ConfigFactId,
};
use upwell_di::{ComponentDescriptor, ProviderDescriptor, factories};
use upwell_hooks::HookDescriptor;

use crate::test_support::test_component;
use crate::{App, AppBuilder, ProtocolDefinition};

/// The trait both provider components implement and the dependency side resolves as
/// `Arc<dyn Authenticator>`.
pub(super) trait Authenticator: Send + Sync {
    fn scheme(&self) -> &'static str;
}

pub(super) const AUTH_MODE: ConfigFactId = ConfigFactId::new("AuthConfig", "auth", "mode");

/// The primary is eligible exactly when `auth.mode` is `"primary"`.
static PRIMARY_CONDITION: ConditionDescriptor =
    ConditionDescriptor::config_equals("auth-mode-primary", AUTH_MODE, "primary");

/// The fallback is eligible exactly when the primary is not, so the two conditions
/// are mutually exclusive by construction.
static FALLBACK_CONDITION: ConditionDescriptor =
    ConditionDescriptor::not("auth-mode-fallback", &PRIMARY_CONDITION);

/// The config mode each factory construction resolved, in construction order.
static OBSERVED_MODES: tokio::sync::Mutex<Vec<String>> = tokio::sync::Mutex::const_new(Vec::new());

/// Whether the fallback factory fails, to abort a switch during candidate construction.
static FALLBACK_FAILS: AtomicBool = AtomicBool::new(false);

/// Whether the fallback factory panics, to abort a switch during candidate construction.
static FALLBACK_PANICS: AtomicBool = AtomicBool::new(false);

/// Whether extracting the `auth.mode` fact panics, to abort a switch while evaluating.
static SCALARS_PANIC: AtomicBool = AtomicBool::new(false);

pub(super) fn fail_fallback_factory(fails: bool) {
    FALLBACK_FAILS.store(fails, Ordering::SeqCst);
}

pub(super) fn panic_fallback_factory(panics: bool) {
    FALLBACK_PANICS.store(panics, Ordering::SeqCst);
}

pub(super) fn panic_fact_extraction(panics: bool) {
    SCALARS_PANIC.store(panics, Ordering::SeqCst);
}

/// Serializes this fixture's tests so no concurrent construction pushes into
/// [`OBSERVED_MODES`] between a clear and an exact-list assertion. Held for the entire
/// body of each async test; await-safe, so no std guard ever crosses an await.
static TEST_GUARD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(super) async fn lock_test_guard() -> tokio::sync::MutexGuard<'static, ()> {
    TEST_GUARD.lock().await
}

pub(super) async fn observed_modes() -> Vec<String> {
    OBSERVED_MODES.lock().await.clone()
}

pub(super) async fn clear_observed_modes() {
    OBSERVED_MODES.lock().await.clear();
}

#[derive(Deserialize)]
pub(super) struct AuthConfig {
    pub(super) mode: String,
}

impl ConfigProperties for AuthConfig {
    const NAME: &'static str = "AuthConfig";
}

impl ConditionFacts for AuthConfig {
    fn condition_facts(binding_path: &'static str) -> Vec<ConfigFactDescriptor> {
        vec![ConfigFactDescriptor {
            id: ConfigFactId::new("AuthConfig", binding_path, "mode"),
            kind: ConditionScalarKind::String,
            source: upwell_core::descriptor_source!(),
        }]
    }

    fn condition_scalars(
        &self,
        binding_path: &'static str,
    ) -> Vec<(ConfigFactId, ConditionScalar)> {
        if SCALARS_PANIC.load(Ordering::SeqCst) {
            panic!("the auth.mode fact extraction panicked");
        }

        vec![(
            ConfigFactId::new("AuthConfig", binding_path, "mode"),
            ConditionScalar::string(self.mode.clone()),
        )]
    }
}

/// Declares an `Authenticator` provider component with a fixed scheme, its stable ID,
/// and the config mode its construction observed.
macro_rules! authenticator {
    ($ty:ident, $id:literal, $scheme:literal) => {
        pub(super) struct $ty {
            pub(super) observed_mode: String,
        }

        impl Authenticator for $ty {
            fn scheme(&self) -> &'static str {
                $scheme
            }
        }

        test_component!($ty, $id, provides dyn Authenticator);
    };
}

authenticator!(PrimaryAuthenticator, "auth-primary", "primary");
authenticator!(FallbackAuthenticator, "auth-fallback", "fallback");

/// Records the config mode a provider construction resolved.
async fn observe_mode(cfg: &Cfg<AuthConfig>) -> String {
    let observed_mode = cfg.snapshot().mode.clone();

    OBSERVED_MODES.lock().await.push(observed_mode.clone());

    observed_mode
}

async fn construct_primary(cfg: Cfg<AuthConfig>) -> PrimaryAuthenticator {
    let observed_mode = observe_mode(&cfg).await;

    PrimaryAuthenticator { observed_mode }
}

async fn construct_fallback(cfg: Cfg<AuthConfig>) -> Result<FallbackAuthenticator, &'static str> {
    if FALLBACK_FAILS.load(Ordering::SeqCst) {
        return Err("the fallback authenticator is unavailable");
    }

    if FALLBACK_PANICS.load(Ordering::SeqCst) {
        panic!("the fallback authenticator panicked");
    }

    let observed_mode = observe_mode(&cfg).await;

    Ok(FallbackAuthenticator { observed_mode })
}

static PRIMARY_PROVIDER: ProviderDescriptor =
    ProviderDescriptor::of::<PrimaryAuthenticator, dyn Authenticator>("dyn Authenticator")
        .primary();

static FALLBACK_PROVIDER: ProviderDescriptor =
    ProviderDescriptor::of::<FallbackAuthenticator, dyn Authenticator>("dyn Authenticator")
        .primary();

/// The primary provider's descriptor carrying `hooks`.
pub(super) const fn primary_component(
    hooks: fn() -> &'static [HookDescriptor],
) -> ComponentDescriptor {
    ComponentDescriptor::of::<PrimaryAuthenticator>()
        .with_condition(&PRIMARY_CONDITION)
        .with_factories(factories!["static" => construct_primary])
        .with_hooks(hooks)
        .without_retention()
}

pub(super) static PRIMARY_COMPONENT: ComponentDescriptor =
    primary_component(upwell_hooks::no_hooks);

static FALLBACK_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<FallbackAuthenticator>()
    .with_condition(&FALLBACK_CONDITION)
    .with_factories(factories!["static" => construct_fallback])
    .without_retention();

/// Builds a file-backed app with both provider components and the fixed
/// [`AuthGate`](super::gate::AuthGate) consumer registered, the config bound at `auth`,
/// and the availability fact sourced from the same binding. The temp dir is returned so
/// the source file outlives the app and can be rewritten by the test.
pub(super) async fn build_auth_app(mode: &str) -> (TempDir, App<()>) {
    build_auth_app_with(mode, &super::gate::GATE_COMPONENT, &[]).await
}

/// Builds the auth app with `gate` as the fixed consumer descriptor and `extra`
/// components registered after it.
pub(super) async fn build_auth_app_with(
    mode: &str,
    gate: &'static ComponentDescriptor,
    extra: &[&'static ComponentDescriptor],
) -> (TempDir, App<()>) {
    build_auth_app_from(mode, &PRIMARY_COMPONENT, gate, extra).await
}

/// Builds the auth app with `primary` as the primary provider descriptor, `gate` as the
/// fixed consumer descriptor, and `extra` components registered after them.
pub(super) async fn build_auth_app_from(
    mode: &str,
    primary: &'static ComponentDescriptor,
    gate: &'static ComponentDescriptor,
    extra: &[&'static ComponentDescriptor],
) -> (TempDir, App<()>) {
    let dir = auth_config_dir(mode);
    let app = auth_builder::<()>(&config_dir_of(&dir), primary, gate, extra)
        .build()
        .await
        .expect("auth app builds");

    (dir, app)
}

/// A temp dir whose config sets `auth.mode = mode`.
pub(super) fn auth_config_dir(mode: &str) -> TempDir {
    let dir = tempfile::Builder::new()
        .prefix("upwell-auth-switch-")
        .tempdir()
        .expect("create temp dir");

    write_auth_config(&config_dir_of(&dir), mode);

    dir
}

/// The auth app builder over protocol `D`, reading its config from `config_dir`, with both
/// provider components, `gate`, and `extra` registered.
pub(super) fn auth_builder<D: ProtocolDefinition + Default>(
    config_dir: &Path,
    primary: &'static ComponentDescriptor,
    gate: &'static ComponentDescriptor,
    extra: &[&'static ComponentDescriptor],
) -> AppBuilder<D> {
    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(config_dir, &[], ResolverChain::empty())
            .expect("load config");
    let mut builder = App::<D>::builder("auth-provider-switch-test")
        .config_source(manager)
        .config::<AuthConfig>("auth")
        .condition_facts::<AuthConfig>("auth")
        .component_descriptor(primary)
        .component_descriptor(&FALLBACK_COMPONENT)
        .component_descriptor(gate);

    for descriptor in extra {
        builder = builder.component_descriptor(descriptor);
    }

    builder
        .provider_descriptor(&PRIMARY_PROVIDER)
        .provider_descriptor(&FALLBACK_PROVIDER)
}

pub(super) use super::super::config_dir_of;

pub(super) fn write_auth_config(config_dir: &Path, mode: &str) {
    fs::write(
        config_dir.join("application.toml"),
        format!("[auth]\nmode = \"{mode}\"\n"),
    )
    .expect("write auth config");
}
