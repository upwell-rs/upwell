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
