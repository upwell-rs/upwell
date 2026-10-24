use super::*;

const MODE: ConfigFactId = ConfigFactId::new("AuthConfig", "auth", "mode");

static CUSTOM: ConditionDescriptor = ConditionDescriptor::config_equals("custom", MODE, "custom");
static DEFAULT: ConditionDescriptor = ConditionDescriptor::not("default", &CUSTOM);
static EITHER: ConditionDescriptor = ConditionDescriptor::any("either", &[CUSTOM, DEFAULT]);

#[test]
fn builders_produce_the_literal_predicates() {
    assert!(matches!(
        CUSTOM.predicate,
        ConditionPredicate::ConfigEquals {
            fact: MODE,
            expected: ConditionScalarLiteral::String("custom"),
        }
    ));
    assert!(matches!(
        DEFAULT.predicate,
        ConditionPredicate::Not(negated) if negated.id == "custom"
    ));
    assert!(matches!(
        EITHER.predicate,
        ConditionPredicate::Any([custom, default]) if custom.id == "custom" && default.id == "default"
    ));
    assert_eq!(
        ConditionDescriptor::config_bool("on", MODE)
            .predicate
            .kind(),
        ConditionPredicateKind::ConfigBool
    );
    assert_eq!(
        ConditionDescriptor::all("both", &[]).predicate.kind(),
        ConditionPredicateKind::All
    );
    assert_eq!(
        ConditionDescriptor::component_eligible("present", "auth-gate")
            .predicate
            .kind(),
        ConditionPredicateKind::ComponentEligible
    );
}

#[test]
fn builders_record_their_call_site() {
    let line = line!() + 1;
    let condition = ConditionDescriptor::config_bool("enabled", MODE);

    assert_eq!(condition.id, "enabled");
    assert_eq!(condition.source.file, file!());
    assert_eq!(condition.source.line, line);
    assert_eq!(
        CUSTOM.source.file,
        file!(),
        "a static records its declaration"
    );
}

struct AuthSettings {
    custom: bool,
}

struct Gate;

impl ConditionInput for Gate {
    type Value<'a> = Eligibility;

    const INPUT: ConditionInputId = ConditionInputId::Component("gate");

    fn read<'a>(context: ConditionContext<'a>) -> Option<Self::Value<'a>> {
        context.eligible("gate").map(Eligibility)
    }
}

#[test]
fn the_context_answers_only_declared_inputs_by_kind() {
    let auth = AuthSettings { custom: true };
    let enabled = ConditionScalar::Bool(true);
    let inputs = [
        (
            ConditionInputId::Binding(ConfigBindingId::of::<AuthSettings>("auth")),
            ConditionInputValue::Config(Some(&auth)),
        ),
        (
            ConditionInputId::Fact(MODE),
            ConditionInputValue::Fact(&enabled),
        ),
        (Gate::INPUT, ConditionInputValue::Eligible(false)),
    ];
    let context = ConditionContext::new(&inputs);

    assert!(
        context
            .binding::<AuthSettings>("auth")
            .is_some_and(|auth| auth.custom)
    );
    assert!(context.binding::<AuthSettings>("other").is_none());
    assert!(context.binding::<u8>("auth").is_none());
    assert_eq!(context.fact(MODE), Some(&enabled));
    assert_eq!(context.get::<Gate>(), Some(Eligibility(false)));
    assert_eq!(context.eligible("missing"), None);
}

#[test]
fn the_raw_scope_answers_declared_paths_and_records_the_first_rejection() {
    let value: scope::RawConfigValue = std::sync::Arc::new(7_u32);

    let (reads, rejection) =
        scope::with_raw_inputs(vec![("present", Some(value)), ("absent", None)], || {
            let present =
                scope::raw_input("present").and_then(|value| value.downcast_ref::<u32>().copied());
            let absent = scope::raw_input("absent").is_none();
            let undeclared = scope::raw_input("undeclared").is_none();

            scope::reject_input(scope::ConditionInputRejection::Invalid {
                path: "present",
                target: "bool",
            });

            (present, absent, undeclared)
        });

    assert_eq!(reads, (Some(7), true, true));
    assert_eq!(
        rejection,
        Some(scope::ConditionInputRejection::Undeclared("undeclared"))
    );
}

#[test]
fn nested_raw_scopes_restore_the_enclosing_scope() {
    let ((inner, outer_after), rejection) = scope::with_raw_inputs(vec![("outer", None)], || {
        let (inner, _) = scope::with_raw_inputs(vec![("inner", None)], || {
            scope::raw_input("inner").is_none()
        });

        (inner, scope::raw_input("outer").is_none())
    });

    assert!(inner && outer_after);
    assert_eq!(rejection, None);
}

#[test]
#[should_panic(expected = "only readable inside a condition evaluation")]
fn raw_reads_outside_an_evaluation_panic() {
    scope::raw_input("anything");
}
