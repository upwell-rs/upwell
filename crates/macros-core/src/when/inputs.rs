//! Derives a `#[when]` expression's statically declared inputs from its syntax.
//!
//! Only two shapes are inputs, and only they are rewritten; the rest of the expression keeps
//! its exact Rust semantics:
//!
//! - an UpperCamelCase type path used as the receiver of a field access or method call
//!   (`AuthConfig.mode`, `AuditTransport.eligible()`) reads that type's `ConditionInput`;
//! - a call to `resolve("<path>")` declares a raw config path, which must be a literal.
//!
//! Macro bodies that parse as comma-separated expressions (`matches!`, `format!`, …) are
//! lowered the same way.

use proc_macro2::{Span, TokenStream, TokenTree};
use quote::{ToTokens, quote, quote_spanned};
use syn::ext::IdentExt;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::visit_mut::{self, VisitMut};
use syn::{
    Expr, ExprCall, ExprField, ExprLit, ExprMethodCall, ExprPath, Lit, Macro, Member, Token,
};

/// The name of the framework function whose literal argument declares a raw config path.
const RESOLVE_FN: &str = "resolve";

/// One declared input of a lowered expression.
pub(super) enum InputSource {
    /// A `ConditionInput` type, named by its path as written.
    Type(syn::Path),
    /// A raw config path read through `resolve`.
    Raw(syn::LitStr),
}

impl InputSource {
    /// Identifies the input regardless of where it is read.
    fn key(&self) -> String {
        match self {
            Self::Type(path) => format!("type:{}", path.to_token_stream()),
            Self::Raw(path) => format!("raw:{}", path.value()),
        }
    }
}

/// One input read, in source order.
pub(super) struct InputRead {
    /// Index into [`Lowering::inputs`].
    pub(super) input: usize,
    /// The dotted field path read from the input; empty for whole-value reads.
    pub(super) field: String,
    pub(super) span: Span,
}

/// Lowers one expression: rewrites input receivers to typed context reads and collects the
/// declared inputs and their reads.
pub(super) struct Lowering {
    read_input: syn::Path,
    condition_input: syn::Path,
    context: syn::Ident,
    pub(super) inputs: Vec<InputSource>,
    pub(super) reads: Vec<InputRead>,
    errors: Option<syn::Error>,
}

impl Lowering {
    pub(super) fn new(
        read_input: syn::Path,
        condition_input: syn::Path,
        context: syn::Ident,
    ) -> Self {
        Self {
            read_input,
            condition_input,
            context,
            inputs: Vec::new(),
            reads: Vec::new(),
            errors: None,
        }
    }

    /// Lowers `expr` in place, failing with every malformed input found.
    pub(super) fn lower(mut self, expr: &mut Expr) -> syn::Result<Self> {
        self.visit_expr_mut(expr);

        match self.errors.take() {
            Some(errors) => Err(errors),

            None => Ok(self),
        }
    }

    fn record(&mut self, source: InputSource, field: String, span: Span) {
        let key = source.key();
        let existing = self.inputs.iter().position(|input| input.key() == key);

        let input = existing.unwrap_or_else(|| {
            self.inputs.push(source);

            self.inputs.len() - 1
        });

        self.reads.push(InputRead { input, field, span });
    }

    fn error(&mut self, error: syn::Error) {
        match &mut self.errors {
            Some(errors) => errors.combine(error),

            None => self.errors = Some(error),
        }
    }

    /// Replaces an input receiver path with a read of its declared value.
    fn read(&mut self, receiver: &mut Expr, field: String) {
        let Expr::Path(path) = receiver else {
            return;
        };
        let path = path.path.clone();
        let span = path.span();
        let read_input = &self.read_input;
        let condition_input = &self.condition_input;
        let context = &self.context;

        let read = quote!(#read_input(<#path as #condition_input>::read(#context)));

        *receiver = Expr::Verbatim(respan(read, span));

        self.record(InputSource::Type(path), field, span);
    }

    /// Lowers a field-access chain rooted at an input receiver, recording its dotted path.
    fn lower_field(&mut self, expr: &mut Expr) -> bool {
        let Some(field) = input_field_chain(expr) else {
            return false;
        };
        let mut root = expr;

        loop {
            match root {
                Expr::Field(ExprField { base, .. }) => root = base,

                receiver => {
                    self.read(receiver, field);

                    return true;
                }
            }
        }
    }

    fn lower_resolve(&mut self, call: &ExprCall) {
        let path = match call.args.first() {
            Some(Expr::Lit(ExprLit {
                lit: Lit::Str(path),
                ..
            })) if call.args.len() == 1 => path.clone(),

            _ => {
                let message = "`resolve` takes one string-literal config path, so the condition declares it statically";
                let error = if call.args.is_empty() {
                    syn::Error::new_spanned(call, message)
                } else {
                    syn::Error::new_spanned(&call.args, message)
                };

                self.error(error);

                return;
            }
        };
        let span = path.span();

        self.record(InputSource::Raw(path), String::new(), span);
    }
}

impl VisitMut for Lowering {
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        match expr {
            Expr::Field(_) => {
                if !self.lower_field(expr) {
                    visit_mut::visit_expr_mut(self, expr);
                }
            }

            Expr::MethodCall(ExprMethodCall { receiver, args, .. }) => {
                match &**receiver {
                    Expr::Path(path) if is_input_receiver(path) => {
                        self.read(receiver, String::new());
                    }

                    _ => self.visit_expr_mut(receiver),
                }

                for arg in args {
                    self.visit_expr_mut(arg);
                }
            }

            Expr::Call(call) if is_resolve(&call.func) => self.lower_resolve(call),

            _ => visit_mut::visit_expr_mut(self, expr),
        }
    }

    fn visit_macro_mut(&mut self, mac: &mut Macro) {
        let Ok(mut args) = mac.parse_body_with(Punctuated::<Expr, Token![,]>::parse_terminated)
        else {
            return;
        };

        for arg in &mut args {
            self.visit_expr_mut(arg);
        }

        mac.tokens = args.into_token_stream();
    }
}

/// Whether `path` names a type used as a value receiver: a plain path whose last segment is
/// UpperCamelCase, excluding `Type::Variant` / `Type::CONST` paths.
fn is_input_receiver(path: &ExprPath) -> bool {
    let segments = &path.path.segments;
    let Some(last) = segments.last() else {
        return false;
    };
    let parent_is_type = segments.len() > 1
        && segments
            .iter()
            .nth(segments.len() - 2)
            .is_some_and(|parent| is_upper_camel(&parent.ident));

    path.qself.is_none()
        && path.attrs.is_empty()
        && last.arguments.is_none()
        && last.ident != "Self"
        && is_upper_camel(&last.ident)
        && !parent_is_type
}

/// UpperCamelCase as opposed to SCREAMING_CASE constants and snake_case values.
fn is_upper_camel(ident: &syn::Ident) -> bool {
    let name = ident.unraw().to_string();
    let starts_upper = name.chars().next().is_some_and(char::is_uppercase);
    let has_lower = name.chars().any(char::is_lowercase);

    starts_upper && (name.len() == 1 || has_lower)
}

fn is_resolve(func: &Expr) -> bool {
    match func {
        Expr::Path(path) => {
            path.qself.is_none()
                && path
                    .path
                    .segments
                    .last()
                    .is_some_and(|segment| segment.ident == RESOLVE_FN)
        }

        _ => false,
    }
}

/// Locates every token of `tokens` at `span`, keeping each token's own name resolution, so
/// diagnostics about a rewritten receiver point at the user's token rather than at the
/// component macro.
fn respan(tokens: TokenStream, span: Span) -> TokenStream {
    tokens
        .into_iter()
        .map(|mut token| {
            if let TokenTree::Group(group) = &token {
                let mut respanned =
                    proc_macro2::Group::new(group.delimiter(), respan(group.stream(), span));

                respanned.set_span(group.span().located_at(span));
                token = TokenTree::Group(respanned);
            } else {
                token.set_span(token.span().located_at(span));
            }

            token
        })
        .collect()
}

/// The dotted field path of a field-access chain rooted at an input receiver.
fn input_field_chain(expr: &Expr) -> Option<String> {
    let mut members = Vec::new();
    let mut cursor = expr;

    loop {
        match cursor {
            Expr::Field(ExprField { base, member, .. }) => {
                members.push(member_name(member));
                cursor = base;
            }

            Expr::Path(path) if is_input_receiver(path) => break,

            _ => return None,
        }
    }

    members.reverse();

    Some(members.join("."))
}

fn member_name(member: &Member) -> String {
    match member {
        Member::Named(ident) => ident.unraw().to_string(),

        Member::Unnamed(index) => index.index.to_string(),
    }
}

/// The input-identity tokens for one declared input.
pub(super) fn input_id(
    source: &InputSource,
    condition_input: &syn::Path,
    input_id: &syn::Path,
) -> TokenStream {
    match source {
        InputSource::Type(path) => {
            quote_spanned!(path.span()=> <#path as #condition_input>::INPUT)
        }

        InputSource::Raw(path) => quote_spanned!(path.span()=> #input_id::Raw(#path)),
    }
}
