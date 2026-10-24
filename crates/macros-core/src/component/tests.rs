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
