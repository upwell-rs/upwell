use quote::ToTokens;
use syn::ItemImpl;

use crate::extend::NoExt;
use crate::paths::Paths;

use super::{MethodArgs, expand};

fn expansion(item: ItemImpl) -> String {
    expand(MethodArgs::<NoExt>::default(), item, &Paths::upwell())
        .expect("the impl block expands")
        .to_string()
}

#[test]
fn every_recipe_registers_its_role_under_a_method_derived_id() {
    let expanded = expansion(syn::parse_quote! {
        impl Ledger {
            #[replace]
            async fn carry(previous: Previous<Self>) -> Self { Self }

            #[transition]
            fn pick(change: &ComponentChange) -> Option<OneShot<Self>> { None }

            #[hot_update]
            fn entries(previous: Previous<Ledger>) -> HotUpdate<Self> { HotUpdate::new(|_| {}) }
        }
    });

    for (id, role) in [
        ("\"replace:carry\"", "Replacement"),
        ("\"transition:pick\"", "Transition"),
        ("\"hot_update:entries\"", "HotUpdate"),
    ] {
        assert!(expanded.contains(id), "registers {id}");
        assert!(
            expanded.contains(&format!("ComponentFactoryRole :: {role}")),
            "registers the {role} role"
        );
    }
}

#[test]
fn a_rejected_marker_keeps_the_stripped_impl_block() {
    let expanded = expand(
        MethodArgs::<NoExt>::default(),
        syn::parse_quote! {
            impl Ledger {
                #[init]
                #[replace]
                async fn carry(previous: Previous<Self>) -> Self { Self }
            }
        },
        &Paths::upwell(),
    )
    .expect("a marker diagnostic is emitted beside the item");
    let file = syn::parse2::<syn::File>(expanded).expect("the expansion parses");
    let item = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Impl(item) => Some(item.to_token_stream().to_string()),

            _ => None,
        })
        .expect("the impl block is kept");

    assert!(!item.contains("init"), "the markers are stripped: {item}");
    assert!(
        !item.contains("replace"),
        "the markers are stripped: {item}"
    );
}
