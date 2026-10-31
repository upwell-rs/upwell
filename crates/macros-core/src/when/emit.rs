//! Lowers a component's `#[when]` attributes to its static condition descriptor.
//!
//! Each `#[when]` becomes one trusted callback node that runs the expression verbatim over its
//! declared inputs and carries a `ConditionExpression` for tooling. An expression inside the
//! closed subset (see [`super::purity`]) is marked pure, so tooling may evaluate it. Stacked
//! attributes combine under one `All` root. Nodes are identified by a hash of their expression
//! (or an explicit `id`) and ordered by that id, so the order the attributes are written in
//! cannot change the descriptor.

use std::collections::BTreeSet;

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;

use super::args::WhenAttr;
use super::inputs::{self, Lowering};
use super::purity::is_pure;
use crate::paths::Paths;

/// The id of the `All` root that stacked `#[when]` nodes combine under.
pub(super) const ALL_ID: &str = "when";

/// The callback kind every `#[when]` node reports.
const CALLBACK_KIND: &str = "when";

/// The prefix of a content-derived node id.
const CONTENT_ID_PREFIX: &str = "when-";

/// A component's lowered condition: the supporting items and the expression its descriptor
/// stores as `condition`.
pub(crate) struct Condition {
    pub(crate) items: TokenStream,
    pub(crate) root: TokenStream,
}

impl Condition {
    /// Lowers `whens` for a component of `scope` (`None` for the default singleton scope).
    pub(crate) fn lower(
        whens: Vec<WhenAttr>,
        scope: Option<&syn::Path>,
        paths: &Paths,
    ) -> syn::Result<Self> {
        if whens.is_empty() {
            return Ok(Self {
                items: TokenStream::new(),
                root: quote!(::core::option::Option::None),
            });
        }

        let mut nodes = whens
            .into_iter()
            .map(|when| Node::lower(when, paths))
            .collect::<syn::Result<Vec<_>>>()?;

        reject_duplicate_ids(&nodes)?;
        nodes.sort_by(|left, right| left.id.cmp(&right.id));

        let singleton = scope.map(|scope| singleton_assertion(scope, paths));
        let (node_items, descriptors): (Vec<_>, Vec<_>) = nodes
            .iter()
            .enumerate()
            .map(|(index, node)| node.emit(index, paths))
            .unzip();
        let root = root_descriptor(&nodes, descriptors, paths);
        let option = quote!(::core::option::Option);

        Ok(Self {
            items: quote! {
                #singleton

                #(#node_items)*

                #root
            },
            root: quote!(#option::Some(&__UPWELL_CONDITION)),
        })
    }
}

/// One lowered `#[when]`.
struct Node {
    id: String,
    /// Where a duplicate id is reported: the explicit id literal, or the attribute.
    id_span: Span,
    span: Span,
    text: String,
    /// Where a non-`bool` expression is reported.
    expr_span: Span,
    expr: syn::Expr,
    lowering: Lowering,
    pure: bool,
}

impl Node {
    fn lower(when: WhenAttr, paths: &Paths) -> syn::Result<Self> {
        let WhenAttr {
            args,
            span,
            text,
            canonical,
        } = when;
        let mut expr = args.expr;
        let expr_span = expr.span();
        let pure = is_pure(&expr);
        let (id, id_span) = match &args.id {
            Some(id) => (id.value(), id.span()),

            None => (content_id(&canonical), span),
        };

        let lowering = Lowering::new(
            paths.core("condition::read_input"),
            paths.core("ConditionInput"),
            context_ident(),
        )
        .lower(&mut expr)?;

        Ok(Self {
            id,
            id_span,
            span,
            text,
            expr_span,
            expr,
            lowering,
            pure,
        })
    }

    /// The node's callback items, and its `ConditionDescriptor` expression.
    fn emit(&self, index: usize, paths: &Paths) -> (TokenStream, TokenStream) {
        let callback_ty = paths.core("ConditionCallback");
        let context_ty = paths.core("ConditionContext");
        let expression_ty = paths.core("ConditionExpression");
        let input_id_ty = paths.core("ConditionInputId");
        let condition_input = paths.core("ConditionInput");
        let read_ty = paths.core("ConditionInputRead");
        let purity_ty = paths.core("ConditionPurity");
        let option = quote!(::core::option::Option);

        let evaluate = format_ident!("__upwell_when_{index}");
        let callback = format_ident!("__UPWELL_WHEN_{index}");
        let inputs_const = format_ident!("__UPWELL_WHEN_{index}_INPUTS");
        let context = context_ident();
        let expr = &self.expr;
        let text = &self.text;
        let input_ids = self
            .lowering
            .inputs
            .iter()
            .map(|input| inputs::input_id(input, &condition_input, &input_id_ty));
        let input_count = self.lowering.inputs.len();
        let reads = self.lowering.reads.iter().map(|read| {
            let input = read.input;
            let field = &read.field;
            let source = crate::source::descriptor_source(read.span, paths);

            quote! {
                #read_ty {
                    input: #inputs_const[#input],
                    field: #field,
                    source: #source,
                }
            }
        });

        let purity = if self.pure {
            quote!(#purity_ty::Pure)
        } else {
            quote!(#purity_ty::Trusted)
        };

        let outcome = quote_spanned!(self.expr_span=> let __upwell_outcome: bool = #expr;);

        let items = quote! {
            const #inputs_const: [#input_id_ty; #input_count] = [#(#input_ids),*];

            #[allow(unused_variables, non_snake_case)]
            fn #evaluate(#context: #context_ty<'_>) -> bool {
                #outcome

                __upwell_outcome
            }

            static #callback: #callback_ty = #callback_ty {
                kind: #CALLBACK_KIND,
                inputs: &#inputs_const,
                expression: #option::Some(&#expression_ty {
                    text: #text,
                    reads: &[#(#reads),*],
                }),
                purity: #purity,
                evaluate: #evaluate,
            };
        };
        let descriptor = node_descriptor(&self.id, self.span, quote!(Callback(&#callback)), paths);

        (items, descriptor)
    }
}

/// The `__UPWELL_CONDITION` root: the single node itself, or an `All` over every node.
fn root_descriptor(
    nodes: &[Node],
    mut descriptors: Vec<TokenStream>,
    paths: &Paths,
) -> TokenStream {
    let descriptor_ty = paths.core("ConditionDescriptor");

    if descriptors.len() == 1 {
        let descriptor = descriptors.remove(0);

        return quote! {
            static __UPWELL_CONDITION: #descriptor_ty = #descriptor;
        };
    }

    let count = descriptors.len();
    let span = nodes
        .iter()
        .map(|node| node.span)
        .next()
        .unwrap_or_else(Span::call_site);
    let root = node_descriptor(ALL_ID, span, quote!(All(&__UPWELL_WHEN_NODES)), paths);

    quote! {
        static __UPWELL_WHEN_NODES: [#descriptor_ty; #count] = [#(#descriptors),*];

        static __UPWELL_CONDITION: #descriptor_ty = #root;
    }
}

fn node_descriptor(id: &str, span: Span, predicate: TokenStream, paths: &Paths) -> TokenStream {
    let descriptor_ty = paths.core("ConditionDescriptor");
    let predicate_ty = paths.core("ConditionPredicate");
    let source = crate::source::descriptor_source(span, paths);

    quote! {
        #descriptor_ty {
            id: #id,
            source: #source,
            predicate: #predicate_ty::#predicate,
        }
    }
}

/// Rejects two nodes with one id (including the stacked `All` root's), which would make the
/// component's condition decisions ambiguous.
fn reject_duplicate_ids(nodes: &[Node]) -> syn::Result<()> {
    let mut seen = BTreeSet::new();

    if nodes.len() > 1 {
        seen.insert(ALL_ID);
    }

    for node in nodes {
        if !seen.insert(node.id.as_str()) {
            return Err(syn::Error::new(
                node.id_span,
                format!(
                    "duplicate `#[when]` condition id `{}`; give each condition a distinct expression or `id = \"..\"`",
                    node.id
                ),
            ));
        }
    }

    Ok(())
}

/// A `const` assertion that `scope` is the singleton scope, failing at the scope argument.
fn singleton_assertion(scope: &syn::Path, paths: &Paths) -> TokenStream {
    let static_scope = paths.core("Scope");
    let singleton = paths.core("scope::Singleton");

    quote_spanned! {scope.span()=>
        const _: () = ::core::assert!(
            <#scope as #static_scope>::RANK == <#singleton as #static_scope>::RANK,
            "`#[when]` conditions require a singleton component; remove `scope` or the condition",
        );
    }
}

/// The parameter every lowered expression reads its inputs through.
fn context_ident() -> syn::Ident {
    syn::Ident::new("__upwell_condition", Span::mixed_site())
}

/// A stable id derived from the expression's tokens, independent of formatting and
/// declaration order (FNV-1a, 64-bit).
pub(super) fn content_id(canonical: &str) -> String {
    let hash = canonical
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        });

    format!("{CONTENT_ID_PREFIX}{hash:016x}")
}
