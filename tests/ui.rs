//! Compile-fail coverage for every invalid macro form: each case under `tests/ui/<suite>`
//! documents its diagnostic in a `.stderr` snapshot.

#[test]
fn invalid_when_conditions_fail_to_compile() {
    let cases = trybuild::TestCases::new();

    cases.compile_fail("tests/ui/when/*.rs");
}

#[test]
fn invalid_transition_recipes_fail_to_compile() {
    let cases = trybuild::TestCases::new();

    cases.compile_fail("tests/ui/recipes/*.rs");
}

#[test]
fn invalid_entries_fail_to_compile() {
    let cases = trybuild::TestCases::new();

    cases.compile_fail("tests/ui/entries/*.rs");
}

#[test]
fn eager_components_outside_scopes_below_the_root_fail_to_compile() {
    let cases = trybuild::TestCases::new();

    cases.compile_fail("tests/ui/construction/*.rs");
}

#[test]
fn invalid_scope_declarations_fail_to_compile() {
    let cases = trybuild::TestCases::new();

    cases.compile_fail("tests/ui/scopes/*.rs");
}

#[test]
fn unsupported_attributes_on_context_accessors_fail_to_compile() {
    let cases = trybuild::TestCases::new();

    cases.compile_fail("tests/ui/access/*.rs");
}

#[cfg(feature = "rayon")]
#[test]
fn nested_parallel_adapters_after_current_fail_to_compile() {
    let cases = trybuild::TestCases::new();

    cases.compile_fail("tests/ui/rayon/*.rs");
}
