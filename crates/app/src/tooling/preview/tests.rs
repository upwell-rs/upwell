//! What-if previews: flipping a provider's flag plans the switch and its affected fixed
//! consumer from tooling alone, without running factories or trusted callbacks, and every
//! preview stays redacted.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use serde::Deserialize;
use upwell_config::{ConditionFacts, ConfigManager, ConfigProperties, ResolverChain, Toml};
use upwell_core::{
    Cardinality, ConditionCallback, ConditionContext, ConditionDescriptor, ConditionPurity,
    ConditionScalar, ConditionScalarKind, ConfigFactDescriptor, ConfigFactId, DependencyDescriptor,
};
use upwell_di::{Component as _, ComponentDescriptor, ProviderDescriptor};
use upwell_tooling_schema::{ConfigAssignment, PreviewComponent, TransitionPreviewRequest};

use super::*;
use crate::test_support::{TempFixture, test_component, unbuilt_factories};
use crate::{App, ExecutionMode};

static TRUSTED_CALLS: AtomicUsize = AtomicUsize::new(0);
static SCALARS_PANIC: AtomicBool = AtomicBool::new(false);

const CUSTOM_ENABLED: ConfigFactId = ConfigFactId::new("PreviewAuth", "auth", "custom");

trait Authenticator: Send + Sync {}

struct CustomAuthenticator;
struct DefaultAuthenticator;
struct AuthGate;
struct TrustedGated;

impl Authenticator for CustomAuthenticator {}
impl Authenticator for DefaultAuthenticator {}

test_component!(CustomAuthenticator, "preview-custom-auth", provides dyn Authenticator);
test_component!(DefaultAuthenticator, "preview-default-auth", provides dyn Authenticator);
test_component!(AuthGate, "preview-auth-gate");
test_component!(TrustedGated, "preview-trusted-gated");

#[derive(Deserialize)]
struct PreviewAuth {
    custom: bool,
}

impl ConfigProperties for PreviewAuth {
    const NAME: &'static str = "PreviewAuth";
}

impl ConditionFacts for PreviewAuth {
    fn condition_facts(binding_path: &'static str) -> Vec<ConfigFactDescriptor> {
        vec![ConfigFactDescriptor {
            id: ConfigFactId::new("PreviewAuth", binding_path, "custom"),
            kind: ConditionScalarKind::Bool,
            source: upwell_core::descriptor_source!(),
        }]
    }

    fn condition_scalars(
        &self,
        binding_path: &'static str,
    ) -> Vec<(ConfigFactId, ConditionScalar)> {
        if SCALARS_PANIC.load(Ordering::SeqCst) {
            panic!("fact extraction panicked");
        }

        vec![(
            ConfigFactId::new("PreviewAuth", binding_path, "custom"),
            ConditionScalar::Bool(self.custom),
        )]
    }
}

fn trusted(_: ConditionContext<'_>) -> bool {
    TRUSTED_CALLS.fetch_add(1, Ordering::SeqCst);

    true
}

static TRUSTED: ConditionCallback = ConditionCallback {
    kind: "when",
    inputs: &[],
    expression: None,
    purity: ConditionPurity::Trusted,
    evaluate: trusted,
};

static CUSTOM_CONDITION: ConditionDescriptor =
    ConditionDescriptor::config_bool("custom-enabled", CUSTOM_ENABLED);
static TRUSTED_CONDITION: ConditionDescriptor = ConditionDescriptor::callback("trusted", &TRUSTED);

fn gate_dependencies() -> Vec<DependencyDescriptor> {
    vec![upwell_di::dependency_of::<dyn Authenticator>(
        Cardinality::One,
        false,
        false,
    )]
}

static CUSTOM_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<CustomAuthenticator>()
    .with_condition(&CUSTOM_CONDITION)
    .with_factories(unbuilt_factories!("static", Vec::new, Default));
static DEFAULT_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<DefaultAuthenticator>()
    .with_factories(unbuilt_factories!("static", Vec::new, Default));
static GATE_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<AuthGate>()
    .with_factories(unbuilt_factories!("static", gate_dependencies, Default));
static TRUSTED_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<TrustedGated>()
    .with_condition(&TRUSTED_CONDITION)
    .with_factories(unbuilt_factories!("static", Vec::new, Default));

static CUSTOM_PROVIDER: ProviderDescriptor =
    ProviderDescriptor::of::<CustomAuthenticator, dyn Authenticator>("dyn Authenticator").primary();
static DEFAULT_PROVIDER: ProviderDescriptor =
    ProviderDescriptor::of::<DefaultAuthenticator, dyn Authenticator>("dyn Authenticator");

/// A tooling preparation of the file-backed auth app whose `auth.custom` flag is
/// `custom`. The fixture owns the config file for as long as the preparation is used.
fn prepared(custom: bool) -> (TempFixture, PreparedApp<()>) {
    let fixture = TempFixture::new("upwell-preview-");
    let config_dir = fixture.child("config");

    fixture.write(
        "config/application.toml",
        format!("[auth]\ncustom = {custom}\n"),
    );

    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(&config_dir, &[], ResolverChain::empty())
            .expect("load config");
    let prepared = App::<()>::builder("preview-auth")
        .config_source(manager)
        .config::<PreviewAuth>("auth")
        .condition_facts::<PreviewAuth>("auth")
        .component_descriptor(&CUSTOM_COMPONENT)
        .component_descriptor(&DEFAULT_COMPONENT)
        .component_descriptor(&GATE_COMPONENT)
        .component_descriptor(&TRUSTED_COMPONENT)
        .provider_descriptor(&CUSTOM_PROVIDER)
        .provider_descriptor(&DEFAULT_PROVIDER)
        .execution_mode(ExecutionMode::Tooling)
        .prepare()
        .expect("the auth app prepares");

    (fixture, prepared)
}

fn overlay(assignments: &[(&str, &str)]) -> ConfigOverlay {
    let mut overlay = ConfigOverlay::default();

    for (path, value) in assignments {
        overlay.set(path, value).expect("valid assignment");
    }

    overlay
}

fn component<'a>(preview: &'a TransitionPreview, id: &str) -> &'a PreviewComponent {
    preview
        .components
        .iter()
        .find(|component| component.component == id)
        .unwrap_or_else(|| panic!("`{id}` is affected"))
}

#[test]
fn flipping_the_flag_plans_the_provider_switch_and_its_fixed_consumer() {
    let (_fixture, prepared) = prepared(true);
    let before = TRUSTED_CALLS.load(Ordering::SeqCst);

    let preview = prepared.transition_preview(&overlay(&[("auth.custom", "false")]));

    let gate = component(&preview, AuthGate::ID);
    let gate_path = gate.reasons[0]
        .path
        .iter()
        .map(|demand| (demand.consumer.as_str(), demand.requested.as_deref()))
        .collect::<Vec<_>>();

    assert_eq!(preview.outcome, PreviewOutcome::Planned);
    assert_eq!(preview.assignments, ["auth.custom"]);
    assert_eq!(preview.evaluation, PreviewEvaluation::Partial);
    assert_eq!(
        preview.changed,
        [PreviewChangedBinding {
            path: String::from("auth"),
            type_name: String::from("PreviewAuth"),
        }]
    );
    assert_eq!(
        preview.eligibility,
        [EligibilityChange {
            component: String::from(CustomAuthenticator::ID),
            before: PreviewConditionOutcome::Holds,
            after: PreviewConditionOutcome::Fails,
        }]
    );
    assert_eq!(
        component(&preview, CustomAuthenticator::ID).action,
        PreviewAction::Remove
    );
    assert_eq!(component(&preview, CustomAuthenticator::ID).strategy, None);
    assert!(
        preview
            .components
            .iter()
            .all(|component| component.component != DefaultAuthenticator::ID),
        "the unconditional fallback is already constructed: {:?}",
        preview.components
    );
    assert_eq!(gate.action, PreviewAction::Replace);
    assert_eq!(gate.strategy, Some(PreviewStrategy::Reconstruct));
    assert_eq!(
        gate.reasons[0].kind,
        PreviewReasonKind::DependencyTargetChanged
    );
    assert_eq!(
        gate_path,
        [(
            AuthGate::ID,
            Some(std::any::type_name::<dyn Authenticator>())
        )]
    );
    assert_eq!(
        preview.displaced,
        [AuthGate::ID, CustomAuthenticator::ID],
        "consumers retire before the providers they hold"
    );
    assert_eq!(
        TRUSTED_CALLS.load(Ordering::SeqCst),
        before,
        "a preview never runs a trusted callback"
    );
}

#[test]
fn an_unchanged_assignment_is_a_noop() {
    let (_fixture, prepared) = prepared(true);

    let preview = prepared.transition_preview(&overlay(&[("auth.custom", "true")]));

    assert_eq!(preview.outcome, PreviewOutcome::NoOp);
    assert!(preview.changed.is_empty());
    assert!(preview.components.is_empty());
}

#[test]
fn an_unbindable_assignment_is_rejected_while_staging() {
    let (_fixture, prepared) = prepared(true);

    let preview = prepared.transition_preview(&overlay(&[("auth.custom", "sometimes")]));

    assert_eq!(
        preview.outcome,
        PreviewOutcome::Rejected {
            phase: PreviewPhase::Stage,
            failure: PreviewFailure::ConfigBind {
                path: String::from("auth"),
                type_name: String::from("PreviewAuth"),
            },
        }
    );
    assert!(preview.components.is_empty());
}

#[test]
fn a_panicking_fact_extraction_is_rejected_while_evaluating() {
    let (_fixture, prepared) = prepared(true);

    SCALARS_PANIC.store(true, Ordering::SeqCst);

    let preview = prepared.transition_preview(&overlay(&[("auth.custom", "false")]));

    SCALARS_PANIC.store(false, Ordering::SeqCst);

    assert_eq!(
        preview.outcome,
        PreviewOutcome::Rejected {
            phase: PreviewPhase::Evaluate,
            failure: PreviewFailure::Panicked,
        },
        "the phase matches the one a runtime reload of the same change reports"
    );
}

#[test]
fn a_probe_document_carries_the_redacted_preview() {
    let (_fixture, prepared) = prepared(true);
    let request = TransitionPreviewRequest {
        assignments: vec![
            ConfigAssignment {
                path: String::from("auth.custom"),
                value: String::from("false"),
            },
            ConfigAssignment {
                path: String::from("auth.token"),
                value: String::from("\"hunter2\""),
            },
        ],
    };
    let decoded = TransitionPreviewRequest::from_env_value(&request.to_env_value())
        .expect("the request round-trips through its environment value");
    let overlay = super::super::preview_overlay(&decoded).expect("the request is a valid overlay");

    let document = super::super::project_prepared(&prepared, Some(&overlay))
        .expect("the preparation projects");
    let json = document
        .to_canonical_json()
        .expect("the document with a preview is valid");

    assert_eq!(
        document
            .transition_preview
            .as_ref()
            .map(|preview| &preview.outcome),
        Some(&PreviewOutcome::Planned)
    );
    assert!(json.contains("\"transition_preview\""));
    assert!(!json.contains("hunter2"), "values are never serialized");
}

#[test]
fn a_document_without_a_request_has_no_preview() {
    let (_fixture, prepared) = prepared(true);

    let document =
        super::super::project_prepared(&prepared, None).expect("the preparation projects");
    let json = document.to_canonical_json().expect("the document is valid");

    assert_eq!(document.transition_preview, None);
    assert!(!json.contains("transition_preview"));
}
