use quote::quote;

use super::*;

fn expand_str(
    args: proc_macro2::TokenStream,
    item: proc_macro2::TokenStream,
) -> syn::Result<String> {
    let args: ScopeArgs = syn::parse2(args)?;
    let item: ItemStruct = syn::parse2(item).expect("test item parses");
    let paths = args.paths(Paths::upwell());

    expand(args, item, &paths).map(|tokens| tokens.to_string().replace(' ', ""))
}

fn error(args: proc_macro2::TokenStream, item: proc_macro2::TokenStream) -> String {
    match expand_str(args, item) {
        Ok(_) => panic!("the declaration is rejected"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn a_parameterised_scope_derives_its_identity_and_declares_its_fields() {
    let expanded = expand_str(
        quote!(parents = [Root]),
        quote!(
            struct CustomerSync {
                customer: u64,
                region: String,
            }
        ),
    )
    .expect("the scope expands");

    assert!(
        expanded.contains(r#"::upwell::namespaced_id!(::upwell::ScopeId,"app/customer_sync")"#)
    );
    assert!(expanded.contains(r#"constNAME:&'staticstr="CustomerSync""#));
    assert!(expanded.contains("::upwell::rank_below(&[<Rootas::upwell::Scope>::RANK])"));
    assert!(expanded.contains("impl::upwell::OpensUnder<Root>forCustomerSync{}"));
    assert!(expanded.contains(r#".with_fields(&["customer","region"])"#));
    assert!(expanded.contains("<CustomerSyncas::upwell::Scope>::RANK>=1"));
    assert!(!expanded.contains("GENERATIONS"));
}

#[test]
fn explicit_arguments_override_the_derived_constants() {
    let expanded = expand_str(
        quote!(
            parents = [Root, Session],
            generations = durable,
            rank = 40,
            id = "billing/run",
            name = "Billing run"
        ),
        quote!(
            struct Run(u64);
        ),
    )
    .expect("the scope expands");

    assert!(expanded.contains(r#"namespaced_id!(::upwell::ScopeId,"billing/run")"#));
    assert!(expanded.contains("constRANK:u8=40;"));
    assert!(expanded.contains(r#"constNAME:&'staticstr="Billingrun""#));
    assert!(expanded.contains(
        "constGENERATIONS:::upwell::ScopeGenerations=::upwell::ScopeGenerations::Durable;"
    ));
    assert!(expanded.contains("impl::upwell::OpensUnder<Session>forRun{}"));
    assert!(expanded.contains(r#".with_fields(&["0"])"#));
}

#[test]
fn a_generation_policy_path_is_emitted_verbatim() {
    let expanded = expand_str(
        quote!(parents = [Root], generations = my::POLICY),
        quote!(
            struct Unit;
        ),
    )
    .expect("the scope expands");

    assert!(expanded.contains("constGENERATIONS:::upwell::ScopeGenerations=my::POLICY;"));
}

#[test]
fn invalid_declarations_are_rejected() {
    let unit = quote!(
        struct Unit;
    );

    assert!(error(quote!(), unit.clone()).contains("needs every parent"));
    assert!(error(quote!(parents = []), unit.clone()).contains("at least one parent"));
    assert!(
        error(quote!(parents = [Root], rank = 255), unit.clone()).contains("between 1 and 254")
    );
    assert!(
        error(quote!(parents = [Root], id = "Bad"), unit.clone()).contains("not a valid scope ID")
    );
    assert!(
        error(
            quote!(parents = [Root], generations = forever),
            unit.clone()
        )
        .contains("unknown generation policy")
    );
    assert!(error(quote!(parents = [Root], lifetime = 1), unit).contains("unknown argument"));
    assert!(
        error(
            quote!(parents = [Root]),
            quote!(
                struct Generic<T>(T);
            )
        )
        .contains("cannot be generic")
    );
}
