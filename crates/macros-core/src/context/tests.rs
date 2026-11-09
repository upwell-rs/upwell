use crate::paths::Paths;

use super::{ContextArgs, expand};

/// The expansion of `#[derive(Clone)] struct Identity;` under context arguments `args`.
fn expansion(args: &str) -> syn::Result<String> {
    let args = syn::parse_str::<ContextArgs>(args)?;

    expand(
        args,
        syn::parse_quote!(
            #[derive(Clone)]
            struct Identity;
        ),
        Paths::upwell(),
    )
    .map(|tokens| tokens.to_string())
}

#[test]
fn a_context_is_a_by_value_seed_in_each_listed_scope() {
    let tokens = expansion("scopes = [WebsocketConnection, HttpRequest]").expect("expands");

    assert!(
        tokens.contains("construction : :: upwell :: ScopedConstruction :: Context"),
        "{tokens}"
    );
    assert!(tokens.contains("type Handle = Self"), "{tokens}");
    assert!(
        tokens.contains("IS_TRANSIENT"),
        "the scopes are checked: {tokens}"
    );
    assert!(
        tokens.contains("< WebsocketConnection as :: upwell :: Scope > :: KEY")
            && tokens.contains("< HttpRequest as :: upwell :: Scope > :: KEY"),
        "both scopes are declared: {tokens}"
    );
}

#[test]
fn a_context_without_scopes_is_rejected() {
    let error = expansion("").expect_err("a context needs scopes");

    assert!(error.to_string().contains("needs `scopes = [..]`"));
}

#[test]
fn an_unknown_or_repeated_argument_is_rejected() {
    let unknown = expansion("scope = HttpRequest").expect_err("`scope` is not a context key");
    let repeated =
        expansion("scopes = [HttpRequest], scopes = [HttpRequest]").expect_err("repeated");

    assert!(
        unknown
            .to_string()
            .contains("unknown `#[context]` argument `scope`")
    );
    assert!(repeated.to_string().contains("`scopes` is given twice"));
}

#[test]
fn a_context_in_the_root_is_rejected() {
    let error = expansion("scopes = [Request, Root]").expect_err("the root opens no instance");

    assert!(error.to_string().contains("only in scopes below the root"));
}
