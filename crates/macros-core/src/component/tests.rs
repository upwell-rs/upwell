use syn::{ImplItem, Item};

use crate::attr::ComponentArgs;
use crate::extend::NoExt;
use crate::paths::Paths;

use super::expand;

#[test]
fn component_impl_items_follow_trait_order() {
    let args = ComponentArgs::<NoExt>::default();
    let item = syn::parse_quote!(
        struct Example;
    );

    let tokens = expand(args, item, &Paths::upwell()).expect("component expands");
    let file = syn::parse2::<syn::File>(tokens).expect("component expansion parses");
    let component_impl = file
        .items
        .iter()
        .find_map(|item| match item {
            Item::Impl(item_impl)
                if item_impl
                    .trait_
                    .as_ref()
                    .and_then(|(_, path, _)| path.segments.last())
                    .is_some_and(|segment| segment.ident == "Component") =>
            {
                Some(item_impl)
            }

            _ => None,
        })
        .expect("Component impl is generated");
    let ordered_items = component_impl
        .items
        .iter()
        .map(|item| match item {
            ImplItem::Type(item) => format!("type {}", item.ident),
            ImplItem::Const(item) => format!("const {}", item.ident),
            ImplItem::Fn(item) => format!("fn {}", item.sig.ident),
            _ => panic!("unexpected Component impl item"),
        })
        .collect::<Vec<_>>();

    assert_eq!(
        ordered_items,
        ["type Handle", "const ID", "const NAME", "fn into_handle"]
    );
}

#[test]
fn linkme_registration_items_allow_their_required_unsafe_attributes() {
    let tokens = expand(
        ComponentArgs::<NoExt>::default(),
        syn::parse_quote!(
            struct Example;
        ),
        &Paths::upwell(),
    )
    .expect("component expands")
    .to_string();

    assert!(
        tokens.contains("allow (unsafe_code)"),
        "component linkme registration scopes its unsafe lint allowance: {tokens}"
    );
}

#[test]
fn generated_dependency_observation_matches_handle_semantics() {
    let tokens = expand(
        ComponentArgs::<NoExt>::default(),
        syn::parse_quote! {
            struct Example {
                fixed: std::sync::Arc<Fixed>,
                live: Dep<Live>,
                #[config]
                config: Cfg<Config>,
            }
        },
        &Paths::upwell(),
    )
    .expect("component expands")
    .to_string();

    assert!(tokens.contains("DependencyObservation :: Snapshot"));
    assert_eq!(
        tokens.matches("DependencyObservation :: Live").count(),
        2,
        "Dep and Cfg fields are the only live observations: {tokens}"
    );
    assert!(tokens.contains("condition : :: core :: option :: Option :: None"));
}

#[test]
fn retainable_by_value_component_rejects_live_backed_fields() {
    let args = ComponentArgs::<NoExt> {
        by_value: true,
        retainable: true,
        ..ComponentArgs::default()
    };
    let error = expand(
        args,
        syn::parse_quote! {
            struct UnsafeRetention {
                dependency: Option<Dep<LiveService>>,
            }
        },
        &Paths::upwell(),
    )
    .expect_err("known live-backed fields cannot opt into retained cloning");

    assert!(error.to_string().contains("cannot contain Dep<T>"));
}

/// The expansion of `struct Example;` under component arguments `args`.
fn expansion(args: &str) -> String {
    let args = syn::parse_str::<ComponentArgs<NoExt>>(args).expect("component arguments parse");

    expand(
        args,
        syn::parse_quote!(
            struct Example;
        ),
        &Paths::upwell(),
    )
    .expect("component expands")
    .to_string()
}

#[test]
fn an_eager_component_carries_its_policy_and_checks_its_scope() {
    let eager = expansion("scope = Request, eager");
    let on_demand = expansion("scope = Request");

    assert!(eager.contains("construction : :: upwell :: ScopedConstruction :: Eager"));
    assert!(
        eager.contains("IS_TRANSIENT"),
        "the scope is checked: {eager}"
    );
    assert!(on_demand.contains("construction : :: upwell :: ScopedConstruction :: OnDemand"));
    assert!(!on_demand.contains("IS_TRANSIENT"));
}

#[test]
fn a_scope_list_declares_its_first_scope_and_lists_every_one() {
    let listed = expansion("scope = [WebsocketConnection, HttpRequest]");
    let single = expansion("scope = Request");

    assert!(
        listed.contains("type Scope = WebsocketConnection"),
        "the first listed scope is the declared one: {listed}"
    );
    assert!(
        listed.contains("scope : < WebsocketConnection as :: upwell :: Scope > :: KEY"),
        "{listed}"
    );
    assert!(
        listed.contains(
            "scopes : & [< WebsocketConnection as :: upwell :: Scope > :: KEY , \
             < HttpRequest as :: upwell :: Scope > :: KEY]"
        ),
        "the descriptor lists every scope in order: {listed}"
    );
    assert!(single.contains("scopes : & []"), "{single}");
}

#[test]
fn an_eager_scope_list_checks_every_scope() {
    let eager = expansion("scope = [WebsocketConnection, HttpRequest], eager");

    assert_eq!(eager.matches("IS_TRANSIENT").count(), 2, "{eager}");
}

#[test]
fn an_eager_scope_list_naming_the_root_is_rejected_at_that_scope() {
    let error = syn::parse_str::<ComponentArgs<NoExt>>("scope = [HttpRequest, singleton], eager")
        .err()
        .expect("an eager list may not name the root");

    assert!(error.to_string().contains("only to scopes below the root"));
}
