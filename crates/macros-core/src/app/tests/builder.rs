//! The builder a named application assembles.

use super::*;

/// The builder chain `body` expands to, with whitespace removed.
fn builder_chain(body: &str) -> String {
    let source = format!("app Example {{ name: \"example\", protocol: Protocol, {body} }}");
    let input = TokenStream::from_str(&source).expect("test token stream parses");

    expand(parse2::<NamedApp>(input).expect("the app parses"))
        .to_string()
        .replace(' ', "")
}

#[test]
fn deny_unplanned_denies_undeclared_on_demand_builds() {
    let denying = builder_chain("deny_unplanned: true,");
    let allowing = builder_chain("deny_unplanned: false,");
    let omitted = builder_chain("");

    assert!(denying.contains(".auto_discover().deny_unplanned()"));
    assert!(!allowing.contains("deny_unplanned"));
    assert_eq!(allowing, omitted, "`false` is the default");
}
