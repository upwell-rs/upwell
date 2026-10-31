use proc_macro2::Span;
use quote::format_ident;
use syn::{Ident, LitStr};

use super::*;

fn owner(id: &str) -> syn::Result<String> {
    owner_segment(&LitStr::new(id, Span::call_site())).map(|segment| segment.value())
}

fn handler(name: &str) -> syn::Result<String> {
    let ident: Ident = syn::parse_str(name).expect("test handler name parses");

    handler_segment(&ident).map(|segment| segment.value())
}

#[test]
fn handler_names_normalize_to_snake_case() {
    assert_eq!(handler("list_users").expect("snake case"), "list_users");
    assert_eq!(handler("listUsers").expect("camel case"), "list_users");
    assert_eq!(handler("r#type").expect("raw identifier"), "type");
    assert_eq!(handler("_private").expect("leading underscore"), "private");
}

#[test]
fn owner_ids_normalize_each_part() {
    assert_eq!(owner("calc").expect("plain"), "calc");
    assert_eq!(owner("Calc Service").expect("spaced"), "calc_service");
    assert_eq!(
        owner("Billing/UserAccounts").expect("nested"),
        "billing/user_accounts"
    );
}

#[test]
fn names_without_an_ascii_snake_case_form_are_rejected() {
    let unicode = handler_segment(&format_ident!("grüßen")).map(|segment| segment.value());

    assert!(
        unicode.is_err_and(|error| error.to_string().contains("`grüßen` cannot name an entry"))
    );
    assert!(owner("日本").is_err());
}
