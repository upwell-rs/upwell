//! Regression coverage for reload invalidation and panic recovery.
#![cfg(feature = "daemon")]
#![allow(dead_code)]

use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Deserializer};
use tempfile::TempDir;
use upwell::config::Toml;
use upwell::daemon::App;
use upwell::{
    AppError, Cfg, CfgNext, ConfigManager, ConfigProperties, ConfigReload, ConfigReloadError,
    HookOutcome, component, config, methods,
};
use upwell_config::{Resolver, ResolverChain};

fn temp_config(tag: &str, contents: &str) -> (TempDir, PathBuf) {
    let root = tempfile::Builder::new()
        .prefix(&format!("upwell-reload-hardening-{tag}-"))
        .tempdir()
        .expect("create config directory");
    let config = root.path().join("application.toml");

    fs::write(&config, contents).expect("write config");

    (root, config)
}

#[config]
#[derive(Deserialize)]
struct ReferencingConfig {
    url: String,
}

#[component]
struct ReferencingConsumer {
    #[config("server")]
    config: Cfg<ReferencingConfig>,
}

#[tokio::test]
async fn cross_path_reference_changes_republish_the_dependent_binding() {
    let (root, file) = temp_config(
        "cross-path",
        "[defaults]\nhost = \"10.0.0.1\"\n[server]\nurl = \"${defaults.host}:8080\"\n",
    );
    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(root.path(), &[], ResolverChain::empty())
            .expect("load config");
    let app = App::builder("cross-path-reload")
        .config_source(manager)
        .config::<ReferencingConfig>("server")
        .component::<ReferencingConsumer>()
        .build()
        .await
        .expect("build app");
    let consumer = app
        .container()
        .get::<ReferencingConsumer>()
        .expect("resolve consumer");

    assert_eq!(consumer.config.get().url, "10.0.0.1:8080");
    fs::write(
        &file,
        "[defaults]\nhost = \"10.0.0.2\"\n[server]\nurl = \"${defaults.host}:8080\"\n",
    )
    .expect("update referenced path");

    let report = app.reloader().reload().await.expect("reload");

    assert_eq!(report.changed.len(), 1);
    assert_eq!(report.changed[0].path, "server");
    assert_eq!(consumer.config.get().url, "10.0.0.2:8080");
}

struct MutableResolver(Arc<RwLock<String>>);

impl Resolver for MutableResolver {
    fn resolve(&self, key: &str) -> Option<Cow<'_, str>> {
        (key == "mutable_value").then(|| Cow::Owned(self.0.read().expect("resolver lock").clone()))
    }
}

#[config]
#[derive(Deserialize)]
struct ResolverConfig {
    value: String,
}

#[component]
struct ResolverConsumer {
    #[config("resolved")]
    config: Cfg<ResolverConfig>,
}

#[tokio::test]
async fn resolver_changes_republish_a_binding_without_source_edits() {
    let (root, _) = temp_config("resolver", "[resolved]\nvalue = \"${mutable_value}\"\n");
    let value = Arc::new(RwLock::new("first".to_string()));
    let resolvers = ResolverChain(vec![Box::new(MutableResolver(Arc::clone(&value)))]);
    let manager = ConfigManager::<Toml>::load_in_with_resolvers(root.path(), &[], resolvers)
        .expect("load config");
    let app = App::builder("resolver-reload")
        .config_source(manager)
        .config::<ResolverConfig>("resolved")
        .component::<ResolverConsumer>()
        .build()
        .await
        .expect("build app");
    let consumer = app
        .container()
        .get::<ResolverConsumer>()
        .expect("resolve consumer");

    assert_eq!(consumer.config.get().value, "first");
    *value.write().expect("resolver lock") = "second".to_string();

    let report = app.reloader().reload().await.expect("reload");

    assert_eq!(report.changed.len(), 1);
    assert_eq!(consumer.config.get().value, "second");
}

#[config]
#[derive(Deserialize)]
struct ChangedHookConfig {
    value: u32,
}

#[config]
#[derive(Deserialize)]
struct UnchangedHookConfig {
    value: u32,
}

#[component]
struct MultiConfigHook {
    #[default]
    unchanged_seen: AtomicUsize,
}

#[methods]
impl MultiConfigHook {
    #[hook(ConfigReload)]
    async fn on_reload(
        &self,
        #[config("changed")] _changed: CfgNext<ChangedHookConfig>,
        #[config("unchanged")] unchanged: CfgNext<UnchangedHookConfig>,
    ) -> upwell::daemon::Result<HookOutcome> {
        self.unchanged_seen
            .store(unchanged.value as usize, Ordering::SeqCst);

        Ok(HookOutcome::Reloaded)
    }
}

#[tokio::test]
async fn reload_hook_can_read_an_unchanged_staged_binding() {
    let (root, file) = temp_config(
        "unchanged-hook-param",
        "[changed]\nvalue = 1\n[unchanged]\nvalue = 9\n",
    );
    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(root.path(), &[], ResolverChain::empty())
            .expect("load config");
    let app = App::builder("unchanged-hook-param")
        .config_source(manager)
        .config::<ChangedHookConfig>("changed")
        .config::<UnchangedHookConfig>("unchanged")
        .component::<MultiConfigHook>()
        .build()
        .await
        .expect("build app");

    fs::write(&file, "[changed]\nvalue = 2\n[unchanged]\nvalue = 9\n").expect("change one binding");

    app.reloader()
        .reload()
        .await
        .expect("unchanged CfgNext parameter remains available");

    let hook = app
        .container()
        .get::<MultiConfigHook>()
        .expect("the published hook component resolves");

    assert_eq!(hook.unchanged_seen.load(Ordering::SeqCst), 9);
}

struct AdvancingResolver(Arc<AtomicUsize>);

impl Resolver for AdvancingResolver {
    fn resolve(&self, key: &str) -> Option<Cow<'_, str>> {
        if key != "advancing_value" {
            return None;
        }

        let call = self.0.fetch_add(1, Ordering::SeqCst);
        let value = ["first", "second", "third"]
            .get(call)
            .copied()
            .unwrap_or("third");

        Some(Cow::Borrowed(value))
    }
}

#[tokio::test]
async fn committed_snapshot_comes_from_the_same_resolver_pass_as_the_value() {
    let (root, _) = temp_config("exact-pass", "[resolved]\nvalue = \"${advancing_value}\"\n");
    let calls = Arc::new(AtomicUsize::new(0));
    let resolvers = ResolverChain(vec![Box::new(AdvancingResolver(Arc::clone(&calls)))]);
    let manager = ConfigManager::<Toml>::load_in_with_resolvers(root.path(), &[], resolvers)
        .expect("load config");
    let app = App::builder("exact-pass-reload")
        .config_source(manager)
        .config::<ResolverConfig>("resolved")
        .component::<ResolverConsumer>()
        .build()
        .await
        .expect("build app");
    let consumer = app
        .container()
        .get::<ResolverConsumer>()
        .expect("resolve consumer");

    assert_eq!(consumer.config.get().value, "first");

    let first = app.reloader().reload().await.expect("first reload");

    assert_eq!(first.changed.len(), 1);
    assert_eq!(consumer.config.get().value, "third");

    let second = app.reloader().reload().await.expect("second reload");

    assert!(
        second.changed.is_empty(),
        "the stable third resolver value matches the exact committed pass"
    );
    assert_eq!(consumer.config.get().value, "third");
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

struct PanicConfig {
    value: u32,
}

impl<'de> Deserialize<'de> for PanicConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            value: u32,
        }

        let raw = Raw::deserialize(deserializer)?;

        assert_ne!(raw.value, 2, "sensitive user panic payload");

        Ok(Self { value: raw.value })
    }
}

impl ConfigProperties for PanicConfig {
    const NAME: &'static str = "PanicConfig";
}

#[component]
struct PanicConsumer {
    #[config("panic")]
    config: Cfg<PanicConfig>,
}

#[tokio::test]
async fn panicking_deserializer_does_not_poison_future_reloads() {
    let (root, file) = temp_config("panic", "[panic]\nvalue = 1\n");
    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(root.path(), &[], ResolverChain::empty())
            .expect("load config");
    let app = App::builder("panic-reload")
        .config_source(manager)
        .config::<PanicConfig>("panic")
        .component::<PanicConsumer>()
        .build()
        .await
        .expect("build app");
    let consumer = app
        .container()
        .get::<PanicConsumer>()
        .expect("resolve consumer");

    fs::write(&file, "[panic]\nvalue = 2\n").expect("write panicking value");
    let error = app.reloader().reload().await.unwrap_err();

    assert!(
        matches!(&error, AppError::ConfigReload(error) if matches!(**error, ConfigReloadError::Panicked)),
        "unexpected reload error: {error:?}"
    );
    assert!(!error.to_string().contains("sensitive"));
    assert_eq!(
        consumer.config.get().value,
        1,
        "failed reload did not commit"
    );

    fs::write(&file, "[panic]\nvalue = 3\n").expect("write valid value");
    let report = app
        .reloader()
        .reload()
        .await
        .expect("later reload recovers");

    assert_eq!(report.changed.len(), 1);
    assert_eq!(consumer.config.get().value, 3);
}

#[config]
#[derive(Deserialize)]
struct HookPanicConfig {
    value: u32,
}

/// Counts every [`PanicOnceHook`] run across instances: each reload attempt runs the hook
/// on a freshly constructed candidate, so per-instance state cannot express "first run".
static PANIC_ONCE_HOOK_CALLS: AtomicUsize = AtomicUsize::new(0);

#[component]
struct PanicOnceHook {
    #[config("hooked")]
    config: Cfg<HookPanicConfig>,
}

#[methods]
impl PanicOnceHook {
    #[hook(ConfigReload)]
    async fn on_reload(
        &self,
        #[config("hooked")] _next: CfgNext<HookPanicConfig>,
    ) -> upwell::daemon::Result<HookOutcome> {
        if PANIC_ONCE_HOOK_CALLS.fetch_add(1, Ordering::SeqCst) == 0 {
            panic!("sensitive hook panic payload");
        }

        Ok(HookOutcome::Reloaded)
    }
}

#[tokio::test]
async fn panicking_reload_hook_does_not_disable_later_reloads() {
    let (root, file) = temp_config("hook-panic", "[hooked]\nvalue = 1\n");
    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(root.path(), &[], ResolverChain::empty())
            .expect("load config");
    let app = App::builder("hook-panic-reload")
        .config_source(manager)
        .config::<HookPanicConfig>("hooked")
        .component::<PanicOnceHook>()
        .build()
        .await
        .expect("build app");
    let component = app
        .container()
        .get::<PanicOnceHook>()
        .expect("resolve component");

    fs::write(&file, "[hooked]\nvalue = 2\n").expect("write first update");
    let error = app.reloader().reload().await.unwrap_err();

    assert!(
        matches!(&error, AppError::ConfigReload(error) if matches!(**error, ConfigReloadError::Hook { .. })),
        "unexpected reload error: {error:?}"
    );
    assert!(!error.to_string().contains("sensitive"));
    assert_eq!(
        component.config.get().value,
        1,
        "panicking hook aborted commit"
    );

    fs::write(&file, "[hooked]\nvalue = 3\n").expect("write second update");
    app.reloader()
        .reload()
        .await
        .expect("reload subsystem remains usable");

    let published = app
        .container()
        .get::<PanicOnceHook>()
        .expect("the published component resolves");

    assert_eq!(component.config.get().value, 3);
    assert_eq!(published.config.get().value, 3);
    assert_eq!(PANIC_ONCE_HOOK_CALLS.load(Ordering::SeqCst), 2);
}
