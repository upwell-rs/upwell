use quote::format_ident;
use syn::Ident;

use super::*;

#[test]
fn snake_case_and_raw_handler_names_form_segments() {
    let raw: Ident = syn::parse_str("r#type").expect("raw identifier parses");

    let plain = handler_segment(&format_ident!("list_users")).expect("snake case is valid");
    let unraw = handler_segment(&raw).expect("the raw prefix is dropped");

    assert_eq!(plain.value(), "list_users");
    assert_eq!(unraw.value(), "type");
}

#[test]
fn names_outside_the_segment_alphabet_are_rejected() {
    let camel = handler_segment(&format_ident!("listUsers")).map(|segment| segment.value());
    let leading = handler_segment(&format_ident!("_private")).map(|segment| segment.value());

    assert!(camel.is_err_and(|error| {
        error
            .to_string()
            .contains("`listUsers` cannot name an entry")
    }));
    assert!(leading.is_err_and(|error| {
        error
            .to_string()
            .contains("`_private` cannot name an entry")
    }));
}
