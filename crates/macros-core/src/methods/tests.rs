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

#[test]
fn a_scoped_init_registers_its_scope_under_a_name_of_its_own() {
    let expanded = expansion(syn::parse_quote! {
        impl Store {
            #[init(scope = Connection)]
            fn from_connection() -> Self { Self }
        }
    });

    for needle in [
        "__upwell_init_from_connection",
        "< Connection as",
        "Scope > :: KEY",
    ] {
        assert!(
            expanded.contains(needle),
            "a scoped init emits `{needle}`: {expanded}"
        );
    }
}

#[test]
fn a_listed_init_registers_one_factory_for_every_scope() {
    let expanded = expansion(syn::parse_quote! {
        impl Store {
            #[init(scope = [WebsocketConnection, request])]
            fn per_call() -> Self { Self }
        }
    });

    assert_eq!(
        expanded.matches("ComponentFactoryRole :: Explicit").count(),
        1,
        "one factory carries the whole list: {expanded}"
    );

    for needle in [
        "__upwell_init_per_call",
        "scopes : & [< WebsocketConnection as",
        "< Request as",
    ] {
        assert!(
            expanded.contains(needle),
            "a listed init emits `{needle}`: {expanded}"
        );
    }
}

#[test]
fn a_repeated_init_scope_is_rejected_at_the_repetition() {
    let expanded = expansion(syn::parse_quote! {
        impl Store {
            #[init(scope = [Request, Request])]
            fn build() -> Self { Self }
        }
    });

    assert!(
        expanded.contains("`Request` is listed twice"),
        "a repeated scope is a compile error: {expanded}"
    );
}

#[test]
fn an_unscoped_init_keeps_the_inherited_scope_and_the_uniqueness_guard() {
    let expanded = expansion(syn::parse_quote! {
        impl Store {
            #[init]
            fn build() -> Self { Self }
        }
    });

    assert!(
        expanded.contains("async fn init"),
        "the fixed-name wrapper stays the uniqueness guard: {expanded}"
    );
    assert!(
        expanded.contains("scopes : & []"),
        "the factory inherits the component scope: {expanded}"
    );
}

#[test]
fn bound_init_parameters_are_wrapped_and_stripped() {
    let expanded = expansion(syn::parse_quote! {
        impl Store {
            #[init]
            fn build(
                #[config("a.b")] cfg: Cfg<Settings>,
                #[qualifier = "main"] backend: Arc<dyn Backend>,
            ) -> Self { Self }
        }
    });

    for needle in ["# [config", "# [qualifier"] {
        assert!(
            !expanded.contains(needle),
            "`{needle}` is stripped: {expanded}"
        );
    }

    for needle in ["resolve_path", "resolve_qualified"] {
        assert!(
            expanded.contains(needle),
            "the wrapper calls `{needle}`: {expanded}"
        );
    }
}

#[test]
fn an_unknown_init_argument_is_rejected_at_the_argument() {
    let expanded = expansion(syn::parse_quote! {
        impl Store {
            #[init(priority = 1)]
            fn build() -> Self { Self }
        }
    });

    assert!(
        expanded.contains("unknown `#[init]` argument"),
        "an unknown argument is a compile error: {expanded}"
    );
}
