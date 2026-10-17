//! Compile-fail coverage for every invalid `#[when]` form: each case under `tests/ui/when`
//! documents the diagnostic in its `.stderr` snapshot.

#[test]
fn invalid_when_conditions_fail_to_compile() {
    let cases = trybuild::TestCases::new();

    cases.compile_fail("tests/ui/when/*.rs");
}
