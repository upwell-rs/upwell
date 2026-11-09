use quote::ToTokens;

use super::*;

fn parse(input: &str) -> syn::Result<ScopeList> {
    syn::parse_str(input)
}

fn listed(scopes: &ScopeList) -> Vec<String> {
    scopes
        .paths()
        .iter()
        .map(|path| path.to_token_stream().to_string())
        .collect()
}

#[test]
fn a_single_path_declares_one_scope() {
    let scopes = parse("Request").expect("one path parses");

    assert_eq!(listed(&scopes), ["Request"]);
    assert!(!scopes.is_several());
    assert_eq!(
        scopes.declared_keys(&Paths::upwell()).to_string(),
        "& []",
        "one scope rides on the descriptor's own scope"
    );
}

#[test]
fn a_list_declares_every_scope_in_order() {
    let scopes = parse("[WebsocketConnection, crate::scopes::HttpRequest]").expect("a list parses");
    let keys = scopes.declared_keys(&Paths::upwell()).to_string();

    assert_eq!(
        listed(&scopes),
        ["WebsocketConnection", "crate :: scopes :: HttpRequest"]
    );
    assert_eq!(
        listed(&scopes)[0],
        scopes.primary().to_token_stream().to_string()
    );
    assert!(scopes.is_several());
    assert!(
        keys.find("WebsocketConnection") < keys.find("HttpRequest"),
        "keys keep the declaration order: {keys}"
    );
}

#[test]
fn list_entries_resolve_the_lowercase_aliases() {
    let scopes = parse("[connection, request]").expect("aliases parse");

    assert_eq!(listed(&scopes), ["Connection", "Request"]);
}

#[test]
fn a_one_entry_list_is_one_scope() {
    let scopes = parse("[Request]").expect("a one-entry list parses");

    assert!(!scopes.is_several());
    assert_eq!(scopes.to_token_stream().to_string(), "[Request]");
}

#[test]
fn an_empty_list_is_rejected() {
    let error = parse("[]").err().expect("an empty list is rejected");

    assert!(error.to_string().contains("at least one scope"), "{error}");
}

#[test]
fn a_scope_listed_twice_is_rejected_even_through_an_alias() {
    let error = parse("[Request, request]")
        .err()
        .expect("a repeated scope is rejected");

    assert!(
        error.to_string().contains("`Request` is listed twice"),
        "{error}"
    );
}
