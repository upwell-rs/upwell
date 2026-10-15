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
