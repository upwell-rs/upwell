//! Conditional metadata: every candidate and condition node is projected, the probe never
//! runs a trusted condition callback, and a graph tooling cannot decide names its undecided
//! conditions.

use upwell_core::{ConditionCallback, ConditionContext, ConditionDescriptor, ConditionPurity};
use upwell_tooling_schema::{DiagnosticSeverity, ProbeOutcome, Resource, ToolingDocument};

use super::*;
use crate::test_support::unbuilt_factories;
use crate::{AppBuilder, AppHost, ExecutionMode};

static TRUSTED_CALLS: AtomicUsize = AtomicUsize::new(0);

trait Greeter: Send + Sync {}

struct CustomGreeter;
struct DefaultGreeter;
struct PureGated;
struct NeedsCustom;

impl Greeter for CustomGreeter {}
impl Greeter for DefaultGreeter {}

test_component!(CustomGreeter, "custom-greeter", provides dyn Greeter);
test_component!(DefaultGreeter, "default-greeter", provides dyn Greeter);
test_component!(PureGated, "pure-gated");
test_component!(NeedsCustom, "needs-custom");

fn trusted(_: ConditionContext<'_>) -> bool {
    TRUSTED_CALLS.fetch_add(1, Ordering::SeqCst);

    true
}

fn default_greeter_eligible(context: ConditionContext<'_>) -> bool {
    context
        .eligible(DefaultGreeter::ID)
        .expect("the declared eligibility is supplied")
}

static TRUSTED: ConditionCallback = ConditionCallback {
    kind: "when",
    inputs: &[],
    expression: Some(&upwell_core::ConditionExpression {
        text: "Flags.custom()",
        reads: &[],
    }),
    purity: ConditionPurity::Trusted,
    evaluate: trusted,
};

static PURE: ConditionCallback = ConditionCallback {
    kind: "when",
    inputs: &[upwell_core::ConditionInputId::Component(DefaultGreeter::ID)],
    expression: None,
    purity: ConditionPurity::Pure,
    evaluate: default_greeter_eligible,
};

static CUSTOM_CONDITION: ConditionDescriptor = ConditionDescriptor::callback("custom", &TRUSTED);
static PURE_CONDITION: ConditionDescriptor = ConditionDescriptor::callback("pure", &PURE);

fn needs_custom_dependencies() -> Vec<DependencyDescriptor> {
    vec![upwell_di::dependency_of::<CustomGreeter>(
        Cardinality::One,
        false,
        false,
    )]
}

static CUSTOM_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<CustomGreeter>()
    .with_condition(&CUSTOM_CONDITION)
    .with_factories(unbuilt_factories!("static", Vec::new, Default));
static DEFAULT_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<DefaultGreeter>()
    .with_factories(unbuilt_factories!("static", Vec::new, Default));
static PURE_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<PureGated>()
    .with_condition(&PURE_CONDITION)
    .with_factories(unbuilt_factories!("static", Vec::new, Default));
static NEEDS_CUSTOM_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<NeedsCustom>()
    .with_factories(unbuilt_factories!(
        "static",
        needs_custom_dependencies,
        Default
    ));

static CUSTOM_PROVIDER: ProviderDescriptor =
    ProviderDescriptor::of::<CustomGreeter, dyn Greeter>("dyn Greeter").primary();
static DEFAULT_PROVIDER: ProviderDescriptor =
    ProviderDescriptor::of::<DefaultGreeter, dyn Greeter>("dyn Greeter");

fn greeter_builder(name: &str) -> AppBuilder<()> {
    App::<()>::builder(name)
        .config_source(ConfigManager::<Toml>::empty())
        .component_descriptor(&CUSTOM_COMPONENT)
        .component_descriptor(&DEFAULT_COMPONENT)
        .component_descriptor(&PURE_COMPONENT)
        .provider_descriptor(&CUSTOM_PROVIDER)
        .provider_descriptor(&DEFAULT_PROVIDER)
}

/// The greeter app: a trusted-gated custom provider, an unconditional fallback, and a
/// component gated by a pure callback.
struct GreeterHost;

impl AppHost for GreeterHost {
    type Protocol = ();

    fn builder() -> Result<AppBuilder<()>, upwell_config::ConfigError> {
        Ok(greeter_builder("conditional-greeter"))
    }
}

/// The greeter app plus a consumer that only the trusted-gated provider satisfies.
struct UndecidedHost;

impl AppHost for UndecidedHost {
    type Protocol = ();

    fn builder() -> Result<AppBuilder<()>, upwell_config::ConfigError> {
        Ok(greeter_builder("undecided-greeter").component_descriptor(&NEEDS_CUSTOM_COMPONENT))
    }
}

fn resource<'a>(document: &'a ToolingDocument, id: &str) -> &'a Resource {
    document
        .resources
        .iter()
        .find(|resource| resource.id == id)
        .unwrap_or_else(|| panic!("resource `{id}` is projected"))
}

fn label<'a>(document: &'a ToolingDocument, id: &str, name: &str) -> &'a str {
    resource(document, id)
        .labels
        .get(name)
        .unwrap_or_else(|| panic!("resource `{id}` carries label `{name}`"))
}

async fn probe_document<H: AppHost>() -> ToolingDocument {
    match crate::tooling::probe_host::<H>(probe_identity())
        .await
        .outcome
    {
        ProbeOutcome::Success { document } => *document,
        ProbeOutcome::Failure { failure } => panic!("the probe failed: {failure:?}"),
    }
}

#[tokio::test]
async fn the_probe_never_runs_trusted_condition_callbacks() {
    let before = TRUSTED_CALLS.load(Ordering::SeqCst);

    let document = probe_document::<GreeterHost>().await;

    assert_eq!(TRUSTED_CALLS.load(Ordering::SeqCst), before);
    assert_eq!(
        label(&document, "application", "condition-evaluation"),
        "tooling"
    );
    assert_eq!(
        label(&document, "application", "effective-graph"),
        "partial"
    );
    assert_eq!(
        label(&document, "component:custom-greeter", "condition-state"),
        "not-evaluated"
    );
    assert_eq!(
        label(&document, "component:custom-greeter", "effective"),
        "false"
    );
    assert_eq!(
        label(&document, "component:custom-greeter", "construction"),
        "inactive-candidate"
    );
}

#[tokio::test]
async fn trusted_conditions_show_their_source_and_pure_ones_are_decided() {
    let document = probe_document::<GreeterHost>().await;

    let custom = "condition:custom-greeter:custom";
    let pure = "condition:pure-gated:pure";

    assert_eq!(label(&document, custom, "outcome"), "not-evaluated");
    assert_eq!(label(&document, custom, "purity"), "trusted");
    assert_eq!(label(&document, custom, "expression"), "Flags.custom()");
    assert_eq!(label(&document, pure, "outcome"), "holds");
    assert_eq!(label(&document, pure, "purity"), "pure");
    assert_eq!(
        label(&document, "component:pure-gated", "condition-state"),
        "holds"
    );
    assert!(document.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::Gates
            && relationship.from == custom
            && relationship.to == "component:custom-greeter"
    }));
    assert!(document.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::DependsOn
            && relationship.from == pure
            && relationship.to == "component:default-greeter"
            && relationship.labels.get("role").map(String::as_str) == Some("eligibility-input")
    }));
}

#[tokio::test]
async fn providers_show_eligibility_and_fallback_roles() {
    let document = probe_document::<GreeterHost>().await;

    let custom = super::super::provider_id(&CUSTOM_PROVIDER);
    let fallback = super::super::provider_id(&DEFAULT_PROVIDER);

    assert_eq!(label(&document, &custom, "effective"), "false");
    assert_eq!(label(&document, &custom, "selection-role"), "conditional");
    assert_eq!(label(&document, &fallback, "effective"), "true");
    assert_eq!(label(&document, &fallback, "selection-role"), "fallback");
    assert!(
        label(
            &document,
            "component:default-greeter",
            "transition-capabilities"
        )
        .contains("reconstruct")
    );
    assert!(document.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::Provides
            && relationship.from == "component:custom-greeter"
            && relationship.to == custom
    }));
}

#[tokio::test]
async fn an_undecidable_graph_names_its_unevaluated_conditions() {
    let before = TRUSTED_CALLS.load(Ordering::SeqCst);

    let envelope = crate::tooling::probe_host::<UndecidedHost>(probe_identity()).await;

    let ProbeOutcome::Failure { failure } = envelope.outcome else {
        panic!("the graph without the undecided provider is invalid");
    };

    assert_eq!(TRUSTED_CALLS.load(Ordering::SeqCst), before);
    assert!(failure.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "upwell/tooling-condition-not-evaluated"
            && diagnostic.severity == DiagnosticSeverity::Warning
            && diagnostic.resources == ["component:custom-greeter"]
    }));
    assert!(
        failure
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.severity == DiagnosticSeverity::Error })
    );
}

#[test]
fn runtime_preparation_decides_trusted_conditions() {
    let before = TRUSTED_CALLS.load(Ordering::SeqCst);

    let prepared = greeter_builder("runtime-greeter")
        .prepare()
        .expect("the runtime graph is valid");
    let document = prepared
        .tooling_document()
        .expect("prepared state projects");

    assert!(TRUSTED_CALLS.load(Ordering::SeqCst) > before);
    assert_eq!(
        label(&document, "application", "condition-evaluation"),
        "runtime"
    );
    assert_eq!(
        label(&document, "application", "effective-graph"),
        "complete"
    );
    assert_eq!(
        label(&document, "component:custom-greeter", "condition-state"),
        "holds"
    );
}

#[tokio::test]
async fn an_undecided_tooling_preparation_cannot_be_built() {
    let prepared = greeter_builder("tooling-greeter")
        .execution_mode(ExecutionMode::Tooling)
        .prepare()
        .expect("the decidable graph is valid");

    let error = prepared
        .build()
        .await
        .expect_err("an undecided graph is not the runtime graph");

    assert!(matches!(error, crate::Error::UndecidedConditionsBuild));
}
