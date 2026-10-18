//! Panicking deserializers and reload hooks neither poison nor disable later reloads.

use super::*;

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
        matches!(
            &error,
            AppError::TransitionRejected(rejection)
                if matches!(
                    rejection.error(),
                    AppError::ConfigReload(error) if matches!(**error, ConfigReloadError::Panicked)
                )
        ),
        "unexpected reload error: {error:?}"
    );
    assert!(!error.to_string().contains("sensitive"));
    assert_redacted_report(&error);
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

    assert_eq!(report.changed().len(), 1);
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
        matches!(
            &error,
            AppError::TransitionRejected(rejection)
                if matches!(
                    rejection.error(),
                    AppError::ConfigReload(error) if matches!(**error, ConfigReloadError::Hook { .. })
                )
        ),
        "unexpected reload error: {error:?}"
    );
    assert!(!error.to_string().contains("sensitive"));
    assert_redacted_report(&error);
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
