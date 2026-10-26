use syn::ItemFn;

use super::*;

fn args(tokens: proc_macro2::TokenStream) -> syn::Result<EntryArgs> {
    syn::parse2(tokens)
}

fn expansion(args: EntryArgs, item: ItemFn) -> syn::Result<String> {
    expand(args, item, &Paths::upwell()).map(|tokens| tokens.to_string())
}

fn error(args: EntryArgs, item: ItemFn) -> String {
    expansion(args, item)
        .expect_err("the entry is rejected")
        .to_string()
}

fn scoped() -> EntryArgs {
    args(quote!(scope = Singleton)).expect("scope parses")
}

#[test]
fn the_key_defaults_to_the_function_name_under_the_app_namespace() {
    let expanded = expansion(
        scoped(),
        syn::parse_quote!(
            async fn reconcileTenants(db: Arc<Db>) {}
        ),
    )
    .expect("the entry expands");

    assert!(expanded.contains("struct ReconcileTenants ;"));
    assert!(expanded.contains("\"app/reconcile_tenants\""));
    assert!(expanded.contains("< Singleton as :: upwell :: Scope > :: KEY"));
    assert!(expanded.contains("< Arc < Db > as :: upwell :: FromContainer > :: dependency ()"));
    assert!(expanded.contains("let _ = reconcileTenants"));
}

#[test]
fn an_explicit_key_replaces_the_default() {
    let expanded = expansion(
        args(quote!(scope = Request, key = "app/tenant.sync")).expect("arguments parse"),
        syn::parse_quote!(
            fn sync() {}
        ),
    )
    .expect("the entry expands");

    assert!(expanded.contains("\"app/tenant.sync\""));
    assert!(!expanded.contains("\"app/sync\""));
}

#[test]
fn malformed_arguments_are_rejected() {
    let missing = args(quote!(key = "app/x"))
        .err()
        .map(|error| error.to_string());
    let unknown = args(quote!(scope = Singleton, name = "x"))
        .err()
        .map(|error| error.to_string());
    let one_segment = args(quote!(scope = Singleton, key = "reconcile"))
        .err()
        .map(|error| error.to_string());
    let uppercase = args(quote!(scope = Singleton, key = "app/Reconcile"))
        .err()
        .map(|error| error.to_string());

    assert!(missing.is_some_and(|error| error.contains("needs the scope it runs in")));
    assert!(unknown.is_some_and(|error| error.contains("expected one of: `scope`, `key`")));
    assert!(
        one_segment.is_some_and(|error| error.contains("`reconcile` is not a valid entry key"))
    );
    assert!(
        uppercase.is_some_and(|error| error.contains("`app/Reconcile` is not a valid entry key"))
    );
}

#[test]
fn functions_that_cannot_be_entries_are_rejected() {
    let generic = error(
        scoped(),
        syn::parse_quote!(
            fn run<T>(value: Arc<T>) {}
        ),
    );
    let receiver = error(
        scoped(),
        syn::parse_quote!(
            fn run(&self) {}
        ),
    );
    let camel = error(
        scoped(),
        syn::parse_quote!(
            fn Run() {}
        ),
    );

    assert!(generic.contains("cannot be generic"));
    assert!(receiver.contains("cannot take `self`"));
    assert!(camel.contains("names its marker type `Run` after this function"));
}

#[test]
fn only_concrete_arc_targets_are_checked_at_compile_time() {
    let paths = Paths::upwell();
    let checked = |ty: Type| checked_bound(&ty, &paths).map(|tokens| tokens.to_string());

    assert_eq!(
        checked(syn::parse_quote!(Arc<Db>)).as_deref(),
        Some(
            ":: upwell :: Wiring : :: upwell :: Provided << Arc < Db > as :: upwell :: Injectable > :: Target >"
        )
    );
    assert_eq!(checked(syn::parse_quote!(Arc<dyn Store>)), None);
    assert_eq!(checked(syn::parse_quote!(Cfg<Limits>)), None);
    assert_eq!(checked(syn::parse_quote!(Option<Arc<Db>>)), None);
    assert_eq!(checked(syn::parse_quote!(Vec<Arc<dyn Store>>)), None);
}
