use syn::parse_quote;

use super::*;

fn parse(attr: syn::Attribute) -> syn::Result<HookAttr> {
    parse_hook_attr(&attr)
}

#[test]
fn parses_the_kind_and_each_flag() {
    let plain = parse(parse_quote!(#[hook(Startup)])).expect("a bare kind parses");
    let observer = parse(parse_quote!(#[hook(upwell::Startup, observe)]))
        .expect("a kind with the observe flag parses");
    let both =
        parse(parse_quote!(#[hook(Vote, always, observe)])).expect("flags parse in any order");

    assert!(plain.kind.is_ident("Startup"));
    assert!(!plain.observe && !plain.always);
    assert_eq!(observer.kind.segments.len(), 2);
    assert!(observer.observe && !observer.always);
    assert!(both.observe && both.always);
}

#[test]
fn a_bare_flag_is_reported_as_a_missing_kind() {
    for attr in [
        parse_quote!(#[hook(observe)]),
        parse_quote!(#[hook(always, observe)]),
    ] {
        let error = parse(attr)
            .err()
            .expect("a flag in the kind position is rejected");

        assert!(
            error.to_string().starts_with("expected a hook kind before"),
            "unexpected diagnostic: {error}"
        );
    }
}

#[test]
fn unknown_and_duplicate_flags_are_rejected() {
    let unknown = parse(parse_quote!(#[hook(Startup, observer)]))
        .err()
        .expect("an unknown flag is rejected");
    let duplicate = parse(parse_quote!(#[hook(Startup, always, always)]))
        .err()
        .expect("a duplicate flag is rejected");

    assert!(unknown.to_string().starts_with("unknown hook flag"));
    assert_eq!(duplicate.to_string(), "duplicate hook flag");
}
