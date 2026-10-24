//! Default-slot replacement, suppression and duplicate installations.

use super::*;

#[test]
fn replacement_executes_only_the_selected_plugin() {
    reset_default_counters();

    let prepared = App::<DefaultProtocol>::builder("replace-default")
        .auto_discover()
        .with_early_plugin_catalog(
            early_catalog::<DefaultProtocol>(|plugins| {
                plugins.replace(TEST_SLOT, ReplacementPlugin);
            })
            .expect("replacement catalog resolves"),
        )
        .prepare()
        .expect("protocol default is replaced");

    assert!(
        prepared
            .plugin_plan()
            .resolution()
            .plugin(DefaultPlugin::ID)
            .is_none()
    );
    assert!(
        prepared
            .plugin_plan()
            .resolution()
            .plugin(ReplacementPlugin::ID)
            .is_some()
    );
    assert_eq!(DEFAULT_DISCOVERIES.load(Ordering::SeqCst), 0);
    assert_eq!(DEFAULT_CONTRIBUTIONS.load(Ordering::SeqCst), 0);
    assert_eq!(REPLACEMENT_DISCOVERIES.load(Ordering::SeqCst), 1);
    assert_eq!(REPLACEMENT_CONTRIBUTIONS.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn host_static_declarations_are_applied_before_late_configuration() {
    reset_default_counters();

    let (_context, prepared) = crate::prepare_host::<ReplacementHost>(ExecutionMode::Tooling)
        .await
        .expect("static host plugin declarations prepare");

    assert!(
        prepared
            .plugin_plan()
            .resolution()
            .plugin(DefaultPlugin::ID)
            .is_none()
    );
    assert!(
        prepared
            .plugin_plan()
            .resolution()
            .plugin(ReplacementPlugin::ID)
            .is_some()
    );
}

#[test]
fn suppression_never_executes_the_optional_default() {
    reset_default_counters();

    let prepared = App::<DefaultProtocol>::builder("suppress-default")
        .auto_discover()
        .with_early_plugin_catalog(
            early_catalog::<DefaultProtocol>(|plugins| plugins.suppress(TEST_SLOT))
                .expect("suppression catalog resolves"),
        )
        .prepare()
        .expect("optional protocol default is suppressed");

    assert!(prepared.plugin_plan().resolution().plugins().is_empty());
    assert_eq!(DEFAULT_DISCOVERIES.load(Ordering::SeqCst), 0);
    assert_eq!(DEFAULT_CONTRIBUTIONS.load(Ordering::SeqCst), 0);
}

fn early_catalog<D: ProtocolDefinition>(
    declarations: impl FnOnce(&mut crate::plugin::ApplicationPluginRegistrar),
) -> crate::Result<crate::plugin::EarlyPluginCatalog> {
    let mut protocol = ProtocolPluginRegistrar::new(D::ID);
    let mut application = crate::plugin::ApplicationPluginRegistrar::new();

    D::register_plugins(&mut protocol);
    declarations(&mut application);

    crate::plugin::EarlyPluginCatalog::resolve(protocol, application)
}

#[test]
fn late_registration_does_not_implicitly_replace_an_early_default() {
    let prepared = App::<DefaultProtocol>::builder("late-default-installation")
        .register_plugin::<ReplacementPlugin>()
        .prepare()
        .expect("independent late plugin remains monotonic");

    assert!(
        prepared
            .plugin_plan()
            .resolution()
            .plugin(DefaultPlugin::ID)
            .is_some()
    );
    assert!(
        prepared
            .plugin_plan()
            .resolution()
            .plugin(ReplacementPlugin::ID)
            .is_some()
    );
    assert_eq!(
        prepared
            .plugin_plan()
            .resolution()
            .slot(TEST_SLOT)
            .expect("protocol default retains its slot")
            .id(),
        DefaultPlugin::ID
    );
}

fn reset_default_counters() {
    DEFAULT_DISCOVERIES.store(0, Ordering::SeqCst);
    DEFAULT_CONTRIBUTIONS.store(0, Ordering::SeqCst);
    REPLACEMENT_DISCOVERIES.store(0, Ordering::SeqCst);
    REPLACEMENT_CONTRIBUTIONS.store(0, Ordering::SeqCst);
}

#[test]
fn duplicate_plugin_installations_preserve_both_origins() {
    let result = App::<()>::builder("duplicate-plugin")
        .register_plugin::<StatefulPlugin>()
        .register_plugin::<StatefulPlugin>()
        .prepare();
    let Err(crate::Error::Composition(diagnostics)) = result else {
        panic!("duplicate plugin installation must be a composition error");
    };
    let [crate::CompositionDiagnostic::DuplicatePlugin { first, second, .. }] =
        diagnostics.as_slice()
    else {
        panic!("duplicate plugin report must contain both installations");
    };

    assert_eq!(first.ordinal(), 0);
    assert_eq!(second.ordinal(), 1);
}
