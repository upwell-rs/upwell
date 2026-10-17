use quote::{ToTokens, quote};
use syn::{Attribute, Expr, ItemStruct, parse_quote};

use super::args::WhenAttr;
use super::emit::{ALL_ID, content_id};
use super::inputs::{InputSource, Lowering};
use super::*;
use crate::paths::Paths;

fn lower(expr: Expr) -> syn::Result<(Expr, Lowering)> {
    let paths = Paths::upwell();
    let mut expr = expr;
    let context = syn::Ident::new("cx", proc_macro2::Span::call_site());

    let lowering = Lowering::new(
        paths.core("condition::read_input"),
        paths.core("ConditionInput"),
        context,
    )
    .lower(&mut expr)?;

    Ok((expr, lowering))
}

fn type_inputs(lowering: &Lowering) -> Vec<String> {
    lowering
        .inputs
        .iter()
        .map(|input| match input {
            InputSource::Type(path) => path.to_token_stream().to_string(),
            InputSource::Raw(path) => format!("raw:{}", path.value()),
        })
        .collect()
}

fn read_fields(lowering: &Lowering) -> Vec<(usize, &str)> {
    lowering
        .reads
        .iter()
        .map(|read| (read.input, read.field.as_str()))
        .collect()
}

fn when_attrs(item: &ItemStruct) -> Vec<&Attribute> {
    item.attrs
        .iter()
        .filter(|attr| attr.path().is_ident("when"))
        .collect()
}

macro_rules! assert_inputs {
    ($expr:expr, inputs: [$($input:expr),* $(,)?], reads: [$($read:expr),* $(,)?] $(,)?) => {{
        let (_, lowering) = lower(parse_quote!($expr)).expect("the expression lowers");
        let inputs: Vec<&str> = vec![$($input),*];
        let reads: Vec<(usize, &str)> = vec![$($read),*];

        assert_eq!(type_inputs(&lowering), inputs, "inputs of `{}`", stringify!($expr));
        assert_eq!(read_fields(&lowering), reads, "reads of `{}`", stringify!($expr));
    }};
}

#[test]
fn type_receivers_are_inputs_with_their_field_paths() {
    assert_inputs!(AuthConfig.mode == AuthMode::Custom, inputs: ["AuthConfig"], reads: [(0, "mode")]);
    assert_inputs!(AuthConfig.limits.max > 3, inputs: ["AuthConfig"], reads: [(0, "limits.max")]);
    assert_inputs!(AuthConfig.0, inputs: ["AuthConfig"], reads: [(0, "0")]);
    assert_inputs!(AuditTransport.eligible(), inputs: ["AuditTransport"], reads: [(0, "")]);
    assert_inputs!(
        config::AuthConfig.is_beta(),
        inputs: ["config :: AuthConfig"],
        reads: [(0, "")],
    );
}

#[test]
fn repeated_receivers_declare_one_input_and_keep_every_read() {
    assert_inputs!(
        AuthConfig.audit && AuditTransport.eligible() || AuthConfig.mode.is_custom(),
        inputs: ["AuthConfig", "AuditTransport"],
        reads: [(0, "audit"), (1, ""), (0, "mode")],
    );
}

#[test]
fn values_constants_and_variant_paths_are_not_inputs() {
    assert_inputs!(AuthMode::Custom.is_custom(), inputs: [], reads: []);
    assert_inputs!(MAX_RETRIES.min(3) > 1, inputs: [], reads: []);
    assert_inputs!(settings.enabled && helper(), inputs: [], reads: []);
    assert_inputs!(<AuthConfig as Trait>::VALUE.is_set(), inputs: [], reads: []);
}

#[test]
fn literal_resolve_paths_are_raw_inputs() {
    assert_inputs!(
        resolve("features.audit_export") == Some(true),
        inputs: ["raw:features.audit_export"],
        reads: [(0, "")],
    );
    assert_inputs!(
        upwell::condition::resolve::<u32>("limits.max").is_some_and(|max| max > 3),
        inputs: ["raw:limits.max"],
        reads: [(0, "")],
    );
}

#[test]
fn non_literal_resolve_paths_are_rejected() {
    for expr in [
        parse_quote!(resolve(PATH) == Some(true)),
        parse_quote!(resolve::<bool>() == Some(true)),
        parse_quote!(resolve("a", "b") == Some(true)),
    ] {
        let error = lower(expr).err().expect("the path is not a literal");

        assert!(error.to_string().contains("string-literal config path"));
    }
}

#[test]
fn macro_bodies_that_parse_as_expressions_are_lowered() {
    assert_inputs!(
        matches!(AuthConfig.mode, AuthMode::Custom | AuthMode::Beta),
        inputs: ["AuthConfig"],
        reads: [(0, "mode")],
    );
}

#[test]
fn receivers_are_rewritten_to_typed_reads_and_the_rest_is_kept() {
    let (expr, _) = lower(parse_quote!(AuthConfig.mode == AuthMode::Custom && ready()))
        .expect("the expression lowers");

    assert_eq!(
        expr.to_token_stream().to_string(),
        quote!(
            ::upwell::condition::read_input(<AuthConfig as ::upwell::ConditionInput>::read(cx))
                .mode
                == AuthMode::Custom
                && ready()
        )
        .to_string()
    );
}

#[test]
fn attributes_parse_an_optional_explicit_id() {
    let named: Attribute =
        parse_quote!(#[when(id = "custom", AuthConfig.mode == AuthMode::Custom)]);
    let unnamed: Attribute = parse_quote!(#[when(id == "custom")]);

    let named = WhenAttr::parse(&named).expect("named parses");
    let unnamed = WhenAttr::parse(&unnamed).expect("an `id` comparison is an expression");

    assert_eq!(named.args.id.expect("explicit id").value(), "custom");
    assert!(unnamed.args.id.is_none());
}

#[test]
fn malformed_attributes_are_rejected() {
    for attr in [
        parse_quote!(#[when]),
        parse_quote!(#[when = "x"]),
        parse_quote!(#[when(true false)]),
        parse_quote!(#[when(id = "named")]),
        parse_quote!(#[when(id = "named",)]),
    ] {
        let attr: Attribute = attr;

        assert!(
            WhenAttr::parse(&attr).is_err(),
            "{}",
            attr.to_token_stream()
        );
    }
}

#[test]
fn content_ids_ignore_formatting() {
    let compact: Expr = parse_quote!(AuthConfig.audit && AuditTransport.eligible());
    let spaced: Expr = parse_quote!(AuthConfig.audit && AuditTransport.eligible());
    let other: Expr = parse_quote!(AuthConfig.audit || AuditTransport.eligible());

    let id = |expr: &Expr| content_id(&expr.to_token_stream().to_string());

    assert_eq!(id(&compact), id(&spaced));
    assert_ne!(id(&compact), id(&other));
    assert!(id(&compact).starts_with("when-"));
}

#[test]
fn take_consumes_whens_and_leaves_the_lowered_marker() {
    let mut item: ItemStruct = parse_quote! {
        #[doc = "kept"]
        #[when(AuthConfig.audit)]
        #[upwell::when(resolve("x") == Some(1))]
        struct Gated;
    };

    let whens = take(&mut item.attrs).expect("both parse");

    assert_eq!(whens.len(), 2);
    assert_eq!(item.attrs.len(), 3, "attributes stay in place");

    for attr in item.attrs.iter().skip(1) {
        let syn::Meta::List(list) = &attr.meta else {
            panic!("the marker is a list attribute");
        };

        assert!(args::is_lowered_marker(&list.tokens));
    }

    assert!(
        take(&mut item.attrs)
            .expect("markers are skipped")
            .is_empty()
    );
    assert_eq!(
        when_attrs(&item).len(),
        1,
        "the unqualified marker keeps its path"
    );
}

#[test]
fn the_attribute_passes_lowered_items_through_and_rejects_the_rest() {
    let item = quote!(
        struct Gated;
    );

    let lowered = expand_attr(args::lowered_marker(), item.clone());
    let misplaced = expand_attr(quote!(true), item.clone());

    assert_eq!(lowered.to_string(), item.to_string());
    assert!(misplaced.to_string().contains("compile_error"));
    assert!(
        misplaced
            .to_string()
            .contains("must be placed below a component macro")
    );
}

#[test]
fn stacked_conditions_lower_identically_in_any_order() {
    let paths = Paths::upwell();
    let first: Attribute = parse_quote!(#[when(AuthConfig.audit)]);
    let second: Attribute = parse_quote!(#[when(resolve("features.export") == Some(true))]);

    let lower = |attrs: [&Attribute; 2]| {
        let whens = attrs
            .into_iter()
            .map(|attr| WhenAttr::parse(attr).expect("parses"))
            .collect();

        Condition::lower(whens, None, &paths)
            .expect("lowers")
            .items
            .to_string()
    };

    assert_eq!(lower([&first, &second]), lower([&second, &first]));
}

#[test]
fn duplicate_ids_are_rejected_including_the_stacked_root() {
    let paths = Paths::upwell();
    let cases: [[Attribute; 2]; 3] = [
        [
            parse_quote!(#[when(id = "a", x())]),
            parse_quote!(#[when(id = "a", y())]),
        ],
        [parse_quote!(#[when(x())]), parse_quote!(#[when(x())])],
        [
            parse_quote!(#[when(id = #ALL_ID, x())]),
            parse_quote!(#[when(y())]),
        ],
    ];

    for attrs in cases {
        let whens = attrs
            .iter()
            .map(|attr| WhenAttr::parse(attr).expect("parses"))
            .collect();

        let Err(error) = Condition::lower(whens, None, &paths) else {
            panic!("duplicate ids are rejected");
        };

        assert!(
            error
                .to_string()
                .contains("duplicate `#[when]` condition id")
        );
    }
}

#[test]
fn no_whens_lower_to_no_condition() {
    let condition = Condition::lower(Vec::new(), None, &Paths::upwell()).expect("lowers");

    assert!(condition.items.is_empty());
    assert_eq!(
        condition.root.to_string(),
        quote!(::core::option::Option::None).to_string()
    );
}
