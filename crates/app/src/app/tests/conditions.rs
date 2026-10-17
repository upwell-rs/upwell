//! Config-fact conditions at the initial build, and redaction of panicking condition
//! inputs.

use super::*;

const FLAG_ENABLED: ConfigFactId = ConfigFactId::new("test::FlagConfig", "flags", "enabled");

static FLAG_CONDITION: ConditionDescriptor = ConditionDescriptor {
    id: "flag-enabled",
    source: upwell_core::descriptor_source!(),
    predicate: ConditionPredicate::ConfigBool(FLAG_ENABLED),
};

#[derive(serde::Deserialize)]
struct FlagConfig {
    enabled: bool,
}

impl ConfigProperties for FlagConfig {
    const NAME: &'static str = "FlagConfig";
}

impl ConditionFacts for FlagConfig {
    fn condition_facts(binding_path: &'static str) -> Vec<ConfigFactDescriptor> {
        vec![ConfigFactDescriptor {
            id: ConfigFactId::new("test::FlagConfig", binding_path, "enabled"),
            kind: ConditionScalarKind::Bool,
            source: upwell_core::descriptor_source!(),
        }]
    }

    fn condition_scalars(
        &self,
        binding_path: &'static str,
    ) -> Vec<(ConfigFactId, ConditionScalar)> {
        vec![(
            ConfigFactId::new("test::FlagConfig", binding_path, "enabled"),
            ConditionScalar::Bool(self.enabled),
        )]
    }
}

/// Factory-backed singleton whose availability keys on the `flags.enabled` fact.
struct FlagComponent;

test_component!(FlagComponent, "flag_component");

async fn flag_component() -> FlagComponent {
    FlagComponent
}

static FLAG_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<FlagComponent>()
    .with_condition(&FLAG_CONDITION)
    .with_factories(factories![default "static" => flag_component])
    .without_retention();

async fn build_flag_app(enabled: bool) -> crate::Result<App<()>> {
    let source = if enabled {
        "[flags]\nenabled = true\n"
    } else {
        "[flags]\nenabled = false\n"
    };

    App::<()>::builder("conditional-fact-test")
        .config_source(ConfigManager::<Toml>::from_str(source).expect("test config parses"))
        .config::<FlagConfig>("flags")
        .condition_facts::<FlagConfig>("flags")
        .component_descriptor(&FLAG_COMPONENT)
        .build()
        .await
}

#[tokio::test]
async fn initial_build_excludes_components_whose_config_fact_is_false() {
    let app = build_flag_app(false).await.expect("disabled app builds");
    let view = app.runtime().view();

    assert!(
        !view
            .resolved_components()
            .iter()
            .any(|component| component.id == FlagComponent::ID)
    );
    assert!(app.container().get::<FlagComponent>().is_none());
    assert_eq!(
        view.condition()
            .evaluation()
            .evaluation()
            .component_eligible(FlagComponent::ID),
        Some(false)
    );
}

#[tokio::test]
async fn initial_build_includes_components_whose_config_fact_is_true() {
    let app = build_flag_app(true).await.expect("enabled app builds");
    let view = app.runtime().view();

    assert!(
        view.resolved_components()
            .iter()
            .any(|component| component.id == FlagComponent::ID)
    );
    assert!(app.container().get::<FlagComponent>().is_some());
    assert_eq!(
        view.condition()
            .evaluation()
            .evaluation()
            .component_eligible(FlagComponent::ID),
        Some(true)
    );
}

// Startup condition preparation runs application extension points — fact-scalar
// extraction, fact-descriptor callbacks, and config condition callbacks — so each
// panic site under test owns its config type and control static, and drives
// `evaluate_startup_conditions` directly over an isolated registry and config slots.
// No control is read by more than one test, so concurrent runs cannot arm another
// test's panic path.

/// Whether the scalar fixture's fact extraction panics when invoked. Read only by
/// [`PanicScalarsConfig::condition_scalars`] and armed only by the scalar test.
static SCALARS_PANIC: AtomicBool = AtomicBool::new(false);

#[derive(serde::Deserialize)]
struct PanicScalarsConfig {
    enabled: bool,
}

impl ConfigProperties for PanicScalarsConfig {
    const NAME: &'static str = "PanicScalarsConfig";
}

impl ConditionFacts for PanicScalarsConfig {
    fn condition_facts(binding_path: &'static str) -> Vec<ConfigFactDescriptor> {
        vec![ConfigFactDescriptor {
            id: ConfigFactId::new("test::PanicScalarsConfig", binding_path, "enabled"),
            kind: ConditionScalarKind::Bool,
            source: upwell_core::descriptor_source!(),
        }]
    }

    fn condition_scalars(
        &self,
        binding_path: &'static str,
    ) -> Vec<(ConfigFactId, ConditionScalar)> {
        if SCALARS_PANIC.load(Ordering::SeqCst) {
            panic!("startup condition fact extraction panicked");
        }

        vec![(
            ConfigFactId::new("test::PanicScalarsConfig", binding_path, "enabled"),
            ConditionScalar::Bool(self.enabled),
        )]
    }
}

/// Whether the descriptor fixture's fact-descriptor callback panics when invoked. Read
/// only by [`PanicDescriptorsConfig::condition_facts`] and armed only by the
/// descriptor test.
static DESCRIPTORS_PANIC: AtomicBool = AtomicBool::new(false);

#[derive(serde::Deserialize)]
struct PanicDescriptorsConfig {
    enabled: bool,
}

impl ConfigProperties for PanicDescriptorsConfig {
    const NAME: &'static str = "PanicDescriptorsConfig";
}

impl ConditionFacts for PanicDescriptorsConfig {
    fn condition_facts(binding_path: &'static str) -> Vec<ConfigFactDescriptor> {
        if DESCRIPTORS_PANIC.load(Ordering::SeqCst) {
            panic!("startup condition fact descriptor callback panicked");
        }

        vec![ConfigFactDescriptor {
            id: ConfigFactId::new("test::PanicDescriptorsConfig", binding_path, "enabled"),
            kind: ConditionScalarKind::Bool,
            source: upwell_core::descriptor_source!(),
        }]
    }

    fn condition_scalars(
        &self,
        binding_path: &'static str,
    ) -> Vec<(ConfigFactId, ConditionScalar)> {
        vec![(
            ConfigFactId::new("test::PanicDescriptorsConfig", binding_path, "enabled"),
            ConditionScalar::Bool(self.enabled),
        )]
    }
}

const CALLBACK_FLAG: ConfigFactId =
    ConfigFactId::new("test::PanicCallbackConfig", "flags", "enabled");

/// Whether the callback fixture's config condition callback panics when evaluated. Read
/// only by [`panic_callback_evaluate`] and armed only by the callback test.
static CALLBACK_PANIC: AtomicBool = AtomicBool::new(false);

fn panic_callback_evaluate(context: ConditionContext<'_>) -> bool {
    if CALLBACK_PANIC.load(Ordering::SeqCst) {
        panic!("startup config condition callback panicked");
    }

    context
        .fact(CALLBACK_FLAG)
        .and_then(ConditionScalar::as_bool)
        .unwrap_or(false)
}

static PANIC_CONDITION_CALLBACK: ConditionCallback = ConditionCallback {
    kind: "test/panic-callback-enabled",
    inputs: &[ConditionInputId::Fact(CALLBACK_FLAG)],
    expression: None,
    evaluate: panic_callback_evaluate,
};

static PANIC_CALLBACK_CONDITION: ConditionDescriptor = ConditionDescriptor {
    id: "panic-callback-enabled",
    source: upwell_core::descriptor_source!(),
    predicate: ConditionPredicate::Callback(&PANIC_CONDITION_CALLBACK),
};

#[derive(serde::Deserialize)]
struct PanicCallbackConfig {
    enabled: bool,
}

impl ConfigProperties for PanicCallbackConfig {
    const NAME: &'static str = "PanicCallbackConfig";
}

impl ConditionFacts for PanicCallbackConfig {
    fn condition_facts(binding_path: &'static str) -> Vec<ConfigFactDescriptor> {
        vec![ConfigFactDescriptor {
            id: ConfigFactId::new("test::PanicCallbackConfig", binding_path, "enabled"),
            kind: ConditionScalarKind::Bool,
            source: upwell_core::descriptor_source!(),
        }]
    }

    fn condition_scalars(
        &self,
        binding_path: &'static str,
    ) -> Vec<(ConfigFactId, ConditionScalar)> {
        vec![(
            ConfigFactId::new("test::PanicCallbackConfig", binding_path, "enabled"),
            ConditionScalar::Bool(self.enabled),
        )]
    }
}

struct PanicCallbackComponent;

test_component!(PanicCallbackComponent, "panic_callback_component");

async fn panic_callback_component() -> PanicCallbackComponent {
    PanicCallbackComponent
}

static PANIC_CALLBACK_COMPONENT: ComponentDescriptor =
    ComponentDescriptor::of::<PanicCallbackComponent>()
        .with_condition(&PANIC_CALLBACK_CONDITION)
        .with_factories(factories![default "static" => panic_callback_component])
        .without_retention();

#[test]
fn panicking_condition_scalar_extraction_returns_the_redacted_preparation_error() {
    SCALARS_PANIC.store(true, Ordering::SeqCst);

    let mut manager = ConfigManager::<Toml>::from_str("[flags]\nenabled = true\n")
        .expect("test config parses")
        .into_dynamic();
    manager.register_binding(ConfigBinding::of::<PanicScalarsConfig>("flags"));

    let (_store, slots) = ConfigStore::build(&manager).expect("config store builds");
    let mut registry = AppRegistry::default();
    registry
        .condition_facts
        .push(ConditionFactSource::of::<PanicScalarsConfig>("flags"));

    let error = crate::app::evaluate_startup_conditions(&registry, &manager, &slots)
        .expect_err("the armed fact extraction must abort the condition stage");

    assert!(
        matches!(error, crate::Error::CandidatePreparationPanicked),
        "expected the redacted preparation panic error, got {error:?}"
    );
}

#[test]
fn panicking_condition_fact_descriptors_return_the_redacted_preparation_error() {
    DESCRIPTORS_PANIC.store(true, Ordering::SeqCst);

    let mut manager = ConfigManager::<Toml>::from_str("[flags]\nenabled = true\n")
        .expect("test config parses")
        .into_dynamic();
    manager.register_binding(ConfigBinding::of::<PanicDescriptorsConfig>("flags"));

    let (_store, slots) = ConfigStore::build(&manager).expect("config store builds");
    let mut registry = AppRegistry::default();
    registry
        .condition_facts
        .push(ConditionFactSource::of::<PanicDescriptorsConfig>("flags"));

    let error = crate::app::evaluate_startup_conditions(&registry, &manager, &slots)
        .expect_err("the armed descriptor callback must abort the condition stage");

    assert!(
        matches!(error, crate::Error::CandidatePreparationPanicked),
        "expected the redacted preparation panic error, got {error:?}"
    );
}

#[test]
fn panicking_config_condition_callback_returns_the_redacted_preparation_error() {
    CALLBACK_PANIC.store(true, Ordering::SeqCst);

    let mut manager = ConfigManager::<Toml>::from_str("[flags]\nenabled = true\n")
        .expect("test config parses")
        .into_dynamic();
    manager.register_binding(ConfigBinding::of::<PanicCallbackConfig>("flags"));

    let (_store, slots) = ConfigStore::build(&manager).expect("config store builds");
    let mut registry = AppRegistry::default();
    registry.components.push(PANIC_CALLBACK_COMPONENT);
    registry
        .condition_facts
        .push(ConditionFactSource::of::<PanicCallbackConfig>("flags"));

    let error = crate::app::evaluate_startup_conditions(&registry, &manager, &slots)
        .expect_err("the armed condition callback must abort the condition stage");

    assert!(
        matches!(error, crate::Error::CandidatePreparationPanicked),
        "expected the redacted preparation panic error, got {error:?}"
    );
}

#[test]
fn ordinary_condition_errors_are_preserved_not_redacted_as_preparation_panics() {
    // The component's condition references the `flags.enabled` fact, but no
    // condition-fact source supplies it: an ordinary condition error, not a panic.
    let mut manager = ConfigManager::<Toml>::from_str("[flags]\nenabled = true\n")
        .expect("test config parses")
        .into_dynamic();
    manager.register_binding(ConfigBinding::of::<FlagConfig>("flags"));

    let (_store, slots) = ConfigStore::build(&manager).expect("config store builds");
    let mut registry = AppRegistry::default();
    registry.components.push(FLAG_COMPONENT);

    let error = crate::app::evaluate_startup_conditions(&registry, &manager, &slots)
        .expect_err("the unsupplied condition fact must fail the condition stage");

    assert!(
        matches!(error, crate::Error::Condition(_)),
        "expected the ordinary condition error, got {error:?}"
    );
}
