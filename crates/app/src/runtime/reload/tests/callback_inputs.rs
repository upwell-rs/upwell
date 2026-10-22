//! The #323 acceptance path: one condition callback reads a typed binding, a raw config
//! path, and another component's eligibility, and its decision follows file-backed config
//! changes in both directions through the transactional reload — including a change to a
//! raw path that no type binds.
//!
//! The components use explicit builder descriptors with unique IDs, so nothing registers
//! into the link-time inventory and these tests cannot interfere with discovery tests.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::Deserialize;
use tempfile::TempDir;
use upwell_config::condition::resolve;
use upwell_config::{ConfigManager, ConfigProperties, ResolverChain, Toml};
use upwell_core::{
    ConditionCallback, ConditionContext, ConditionDescriptor, ConditionInputId, ConditionPurity,
    ConfigBindingId,
};
use upwell_di::{Component as _, ComponentDescriptor, factories};

use crate::App;
use crate::test_support::test_component;

#[derive(Deserialize)]
struct AuditConfig {
    enabled: bool,
}

impl ConfigProperties for AuditConfig {
    const NAME: &'static str = "AuditConfig";
}

struct CallbackTransport;
struct CallbackExporter;
struct CallbackAuditor;

test_component!(CallbackTransport, "callback-transport");
test_component!(CallbackExporter, "callback-exporter");
test_component!(CallbackAuditor, "callback-auditor");

/// How often the auditor's binding-only condition ran, across this module's tests.
static AUDITOR_EVALUATIONS: AtomicUsize = AtomicUsize::new(0);

/// Serializes this module's tests, which share [`AUDITOR_EVALUATIONS`], under the default
/// `cargo test` parallelism. Await-safe, so no std guard crosses an await.
static TEST_GUARD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const EXPORT: &str = "features.export";
const TRANSPORT: &str = "transport.enabled";

fn transport_enabled(_: ConditionContext<'_>) -> bool {
    resolve::<bool>(TRANSPORT) == Some(true)
}

fn exporter_enabled(context: ConditionContext<'_>) -> bool {
    context
        .binding::<AuditConfig>("audit")
        .is_some_and(|audit| audit.enabled)
        && resolve::<bool>(EXPORT) == Some(true)
        && context.eligible(CallbackTransport::ID) == Some(true)
}

fn auditor_enabled(context: ConditionContext<'_>) -> bool {
    AUDITOR_EVALUATIONS.fetch_add(1, Ordering::SeqCst);

    context
        .binding::<AuditConfig>("audit")
        .is_some_and(|audit| audit.enabled)
}

static AUDITOR_CALLBACK: ConditionCallback = ConditionCallback {
    kind: "test/auditor-enabled",
    inputs: &[ConditionInputId::Binding(
        ConfigBindingId::of::<AuditConfig>("audit"),
    )],
    expression: None,
    purity: ConditionPurity::Trusted,
    evaluate: auditor_enabled,
};

static AUDITOR_CONDITION: ConditionDescriptor =
    ConditionDescriptor::callback("auditor-enabled", &AUDITOR_CALLBACK);

static TRANSPORT_CALLBACK: ConditionCallback = ConditionCallback {
    kind: "test/transport-enabled",
    inputs: &[ConditionInputId::Raw(TRANSPORT)],
    expression: None,
    purity: ConditionPurity::Trusted,
    evaluate: transport_enabled,
};

static EXPORTER_CALLBACK: ConditionCallback = ConditionCallback {
    kind: "test/exporter-enabled",
    inputs: &[
        ConditionInputId::Binding(ConfigBindingId::of::<AuditConfig>("audit")),
        ConditionInputId::Raw(EXPORT),
        ConditionInputId::Component(CallbackTransport::ID),
    ],
    expression: None,
    purity: ConditionPurity::Trusted,
    evaluate: exporter_enabled,
};

static TRANSPORT_CONDITION: ConditionDescriptor =
    ConditionDescriptor::callback("transport-enabled", &TRANSPORT_CALLBACK);

static EXPORTER_CONDITION: ConditionDescriptor =
    ConditionDescriptor::callback("exporter-enabled", &EXPORTER_CALLBACK);

async fn construct_transport() -> CallbackTransport {
    CallbackTransport
}

async fn construct_exporter() -> CallbackExporter {
    CallbackExporter
}

async fn construct_auditor() -> CallbackAuditor {
    CallbackAuditor
}

static AUDITOR_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<CallbackAuditor>()
    .with_condition(&AUDITOR_CONDITION)
    .with_factories(factories!["static" => construct_auditor]);

static TRANSPORT_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<CallbackTransport>()
    .with_condition(&TRANSPORT_CONDITION)
    .with_factories(factories!["static" => construct_transport]);

static EXPORTER_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<CallbackExporter>()
    .with_condition(&EXPORTER_CONDITION)
    .with_factories(factories!["static" => construct_exporter]);

/// The config inputs one test step writes.
#[derive(Clone, Copy)]
struct Inputs {
    audit: bool,
    export: bool,
    transport: bool,
}

const ALL_ON: Inputs = Inputs {
    audit: true,
    export: true,
    transport: true,
};

use super::config_dir_of;

fn write_inputs(config_dir: &Path, inputs: Inputs) {
    let Inputs {
        audit,
        export,
        transport,
    } = inputs;

    fs::write(
        config_dir.join("application.toml"),
        format!(
            "[audit]\nenabled = {audit}\n\n[features]\nexport = {export}\n\n[transport]\nenabled = {transport}\n"
        ),
    )
    .expect("write callback config");
}

async fn build_app(inputs: Inputs) -> (TempDir, App<()>) {
    let dir = tempfile::Builder::new()
        .prefix("upwell-callback-inputs-")
        .tempdir()
        .expect("create temp dir");
    let config_dir = config_dir_of(&dir);

    write_inputs(&config_dir, inputs);

    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(&config_dir, &[], ResolverChain::empty())
            .expect("load config");
    let app = App::<()>::builder("callback-inputs-test")
        .config_source(manager)
        .config::<AuditConfig>("audit")
        .component_descriptor(&TRANSPORT_COMPONENT)
        .component_descriptor(&EXPORTER_COMPONENT)
        .component_descriptor(&AUDITOR_COMPONENT)
        .build()
        .await
        .expect("callback app builds");

    (dir, app)
}

/// Rewrites the config, reloads, and returns whether the exporter is now active.
async fn reload_with(dir: &TempDir, app: &App<()>, inputs: Inputs) -> bool {
    write_inputs(&config_dir_of(dir), inputs);

    app.runtime()
        .reload_config()
        .await
        .expect("reload succeeds");

    exporter_active(app)
}

fn exporter_active(app: &App<()>) -> bool {
    app.runtime()
        .view()
        .effective_graph()
        .node(CallbackExporter::ID)
        .is_some()
}

#[tokio::test]
async fn a_callback_follows_binding_raw_and_eligibility_inputs_in_both_directions() {
    let _guard = TEST_GUARD.lock().await;
    let (dir, app) = build_app(ALL_ON).await;

    let initial = exporter_active(&app);
    let audit_off = reload_with(
        &dir,
        &app,
        Inputs {
            audit: false,
            ..ALL_ON
        },
    )
    .await;
    let audit_on = reload_with(&dir, &app, ALL_ON).await;
    let transport_off = reload_with(
        &dir,
        &app,
        Inputs {
            transport: false,
            ..ALL_ON
        },
    )
    .await;
    let transport_on = reload_with(&dir, &app, ALL_ON).await;

    assert!(initial, "every input holds at startup");
    assert!(!audit_off, "the typed binding disables the exporter");
    assert!(audit_on, "the typed binding re-enables the exporter");
    assert!(
        !transport_off,
        "an ineligible transport disables the exporter"
    );
    assert!(
        transport_on,
        "an eligible transport re-enables the exporter"
    );
}

#[tokio::test]
async fn a_raw_path_change_without_binding_changes_still_reloads_the_condition() {
    let _guard = TEST_GUARD.lock().await;
    let (dir, app) = build_app(ALL_ON).await;
    let before = app.runtime().generation();

    let export_off = reload_with(
        &dir,
        &app,
        Inputs {
            export: false,
            ..ALL_ON
        },
    )
    .await;
    let after_off = app.runtime().generation();
    let export_on = reload_with(&dir, &app, ALL_ON).await;

    assert!(!export_off, "the raw path disables the exporter");
    assert!(export_on, "the raw path re-enables the exporter");
    assert!(
        after_off > before,
        "a raw-only change publishes a generation"
    );
}

#[tokio::test]
async fn an_unchanged_config_stays_a_no_op_for_raw_path_conditions() {
    let _guard = TEST_GUARD.lock().await;
    let (_dir, app) = build_app(ALL_ON).await;
    let before = app.runtime().generation();

    let report = app
        .runtime()
        .reload_config()
        .await
        .expect("reload succeeds");

    assert!(
        !report.is_committed(),
        "nothing changed, so nothing publishes"
    );
    assert_eq!(app.runtime().generation(), before);
    assert!(exporter_active(&app));
}

#[tokio::test]
async fn only_conditions_reading_a_changed_input_re_evaluate() {
    let _guard = TEST_GUARD.lock().await;
    let (dir, app) = build_app(ALL_ON).await;
    let after_build = AUDITOR_EVALUATIONS.load(Ordering::SeqCst);

    reload_with(
        &dir,
        &app,
        Inputs {
            export: false,
            ..ALL_ON
        },
    )
    .await;
    let after_raw_change = AUDITOR_EVALUATIONS.load(Ordering::SeqCst);

    reload_with(
        &dir,
        &app,
        Inputs {
            audit: false,
            export: false,
            ..ALL_ON
        },
    )
    .await;
    let after_binding_change = AUDITOR_EVALUATIONS.load(Ordering::SeqCst);

    reload_with(
        &dir,
        &app,
        Inputs {
            audit: false,
            ..ALL_ON
        },
    )
    .await;
    let after_committed_binding = AUDITOR_EVALUATIONS.load(Ordering::SeqCst);

    assert_eq!(
        after_raw_change, after_build,
        "a raw-only change leaves the binding-only condition alone"
    );
    assert_eq!(
        after_binding_change,
        after_raw_change + 1,
        "a changed binding re-evaluates its condition once"
    );
    assert_eq!(
        after_committed_binding, after_binding_change,
        "a binding committed by an earlier reload counts as unchanged"
    );
}
