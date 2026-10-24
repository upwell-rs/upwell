use syn::parse_quote;

use super::*;

#[test]
fn type_text_reads_like_source() {
    let ty: Type = parse_quote!(Option<std::collections::BTreeMap<String, Vec<&'static str>>>);

    assert_eq!(
        type_text(&ty),
        "Option<std::collections::BTreeMap<String, Vec<&'static str>>>"
    );
}

#[test]
fn doc_text_joins_lines_without_the_doc_comment_space() {
    let item: DeriveInput = parse_quote! {
        /// The listen address.
        ///
        /// Accepts `${..}` templates.
        struct Http;
    };

    assert_eq!(
        doc_text(&item.attrs),
        "The listen address.\n\nAccepts `${..}` templates."
    );
}

#[test]
fn fields_are_required_unless_optional_or_defaulted() {
    let optional: Type = parse_quote!(Option<u16>);
    let plain: Type = parse_quote!(u16);
    let field: Field = parse_quote!(#[default = "8080"] port: u16);

    assert!(is_option(&optional));
    assert!(!is_option(&plain));
    assert_eq!(default_template(&field.attrs).as_deref(), Some("8080"));
}
