//! Reload invalidation: cross-path references, resolver changes, unchanged staged
//! bindings, and snapshots from the same resolver pass as their values.

use super::*;

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

    assert_eq!(report.changed().len(), 1);
    assert_eq!(report.changed()[0].path, "server");
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

    assert_eq!(report.changed().len(), 1);
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

    assert_eq!(first.changed().len(), 1);
    assert_eq!(consumer.config.get().value, "third");

    let second = app.reloader().reload().await.expect("second reload");

    assert!(
        second.changed().is_empty(),
        "the stable third resolver value matches the exact committed pass"
    );
    assert_eq!(consumer.config.get().value, "third");
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}
