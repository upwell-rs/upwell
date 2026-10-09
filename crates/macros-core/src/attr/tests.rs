use crate::extend::NoExt;

use super::ComponentArgs;

fn parse_error(input: &str) -> syn::Error {
    match syn::parse_str::<ComponentArgs<NoExt>>(input) {
        Ok(_) => panic!("component arguments unexpectedly parsed"),
        Err(error) => error,
    }
}

#[test]
fn retainable_requires_a_by_value_handle() {
    let error = parse_error("retainable");

    assert!(error.to_string().contains("only valid with `by_value`"));

    let args = syn::parse_str::<ComponentArgs<NoExt>>("by_value, retainable")
        .expect("explicit by-value retention parses");

    assert!(args.by_value);
    assert!(args.retainable);

    let scope_error = parse_error("by_value, retainable, scope = Request");

    assert!(scope_error.to_string().contains("singleton-only"));
}

#[test]
fn by_value_component_cannot_register_trait_providers() {
    let error = parse_error("by_value, provide = dyn Repository");

    assert!(
        error
            .to_string()
            .contains("`by_value` cannot be combined with `provide`")
    );
}

#[test]
fn validation_is_independent_of_argument_order() {
    let error = parse_error("provide = [dyn Repository, dyn Cache], by_value");

    assert!(
        error
            .to_string()
            .contains("`by_value` cannot be combined with `provide`")
    );
}

#[test]
fn provider_ordering_accepts_repeated_and_list_targets() {
    let args = syn::parse_str::<ComponentArgs<NoExt>>(
        "provide = [dyn Migration, dyn Audit], before = Create, before = [Index as dyn Migration, Seed as [dyn Migration, dyn Audit]], after = Bootstrap",
    )
    .expect("component arguments parse");

    assert_eq!(args.before.len(), 3);
    assert!(args.before[0].traits.is_empty());
    assert_eq!(args.before[1].traits.len(), 1);
    assert_eq!(args.before[2].traits.len(), 2);
    assert_eq!(args.after.len(), 1);
}

#[test]
fn provider_ordering_rejects_non_trait_restriction() {
    let error = parse_error("provide = dyn Migration, before = Create as Migration");

    assert!(error.to_string().contains("expects a trait object"));
}

#[test]
fn provider_ordering_rejects_empty_lists() {
    let target_error = parse_error("provide = dyn Migration, before = []");
    let trait_error = parse_error("provide = dyn Migration, before = Create as []");

    assert!(
        target_error
            .to_string()
            .contains("target list cannot be empty")
    );
    assert!(
        trait_error
            .to_string()
            .contains("trait list cannot be empty")
    );
}

#[test]
fn provider_ordering_requires_a_provider() {
    let error = parse_error("before = Create");

    assert!(error.to_string().contains("require at least one `provide`"));
}

#[test]
fn provider_priority_accepts_const_expressions() {
    let constant = syn::parse_str::<ComponentArgs<NoExt>>(
        "provide = dyn Migration, priority = COMPONENT_X_PRIO",
    )
    .expect("constant priority parses");
    let associated = syn::parse_str::<ComponentArgs<NoExt>>(
        "provide = dyn Migration, priority = MigrationStep::PRIO",
    )
    .expect("associated constant priority parses");
    let expression = syn::parse_str::<ComponentArgs<NoExt>>(
        "provide = dyn Migration, priority = BASE_PRIO + 10",
    )
    .expect("const expression priority parses");

    assert!(constant.priority.is_some());
    assert!(associated.priority.is_some());
    assert!(expression.priority.is_some());
}

#[test]
fn provider_priority_requires_a_provider() {
    let error = parse_error("priority = COMPONENT_X_PRIO");

    assert!(error.to_string().contains("require at least one `provide`"));
}

#[test]
fn provider_declaration_rejects_duplicate_traits() {
    let error = parse_error("provide = [dyn Migration, dyn Migration]");

    assert!(error.to_string().contains("duplicate trait"));
}
