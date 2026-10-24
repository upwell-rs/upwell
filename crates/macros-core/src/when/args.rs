//! Parsing of one `#[when([id = "..",] <expr>)]` attribute.

use proc_macro2::{Span, TokenStream};
use quote::ToTokens;
use syn::parse::{Parse, ParseStream};
use syn::{Attribute, Expr, Ident, LitStr, MacroDelimiter, Meta, Token};

/// The arguments of one `#[when]`: an optional explicit condition id and the condition
/// expression.
pub(super) struct WhenArgs {
    pub(super) id: Option<LitStr>,
    pub(super) expr: Expr,
}

impl Parse for WhenArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let id = if explicit_id_follows(input) {
            input.parse::<Ident>()?;
            input.parse::<Token![=]>()?;

            let id = input.parse::<LitStr>()?;

            input.parse::<Token![,]>()?;

            Some(id)
        } else {
            None
        };
        let expr = input.parse::<Expr>()?;

        if let Expr::Assign(assign) = &expr {
            return Err(syn::Error::new_spanned(
                assign,
                "expected a boolean condition expression; an explicit id is written `id = \"..\", <expr>`",
            ));
        }

        if !input.is_empty() {
            return Err(input.error(
                "expected a single boolean expression, optionally preceded by `id = \"..\",`",
            ));
        }

        Ok(Self { id, expr })
    }
}

/// Whether the input starts with `id = "<literal>",`, as opposed to an expression that
/// happens to start with an `id` binding.
fn explicit_id_follows(input: ParseStream) -> bool {
    let fork = input.fork();

    let Ok(key) = fork.parse::<Ident>() else {
        return false;
    };

    key == "id"
        && fork.parse::<Token![=]>().is_ok()
        && fork.parse::<LitStr>().is_ok()
        && fork.peek(Token![,])
}

/// One parsed `#[when]` attribute, before lowering.
pub(crate) struct WhenAttr {
    pub(super) args: WhenArgs,
    /// The span of the whole attribute, used as the condition node's source location.
    pub(super) span: Span,
    /// The expression as the user wrote it, for tooling.
    pub(super) text: String,
    /// The expression's whitespace-insensitive token text, which content-derived ids hash.
    pub(super) canonical: String,
}

impl WhenAttr {
    pub(super) fn parse(attr: &Attribute) -> syn::Result<Self> {
        let Meta::List(list) = &attr.meta else {
            return Err(syn::Error::new_spanned(
                attr,
                "`#[when]` takes a boolean condition expression: `#[when(<expr>)]`",
            ));
        };
        let args = syn::parse2::<WhenArgs>(list.tokens.clone())?;
        let canonical = args.expr.to_token_stream().to_string();
        let text = source_text(&list.delimiter, args.id.is_some()).unwrap_or(canonical.clone());

        Ok(Self {
            args,
            span: attr.pound_token.span,
            text,
            canonical,
        })
    }
}

/// The expression's original source text, read from the attribute's parenthesized group
/// with any `id = "..",` prefix removed. `None` when the compiler provides no source text.
fn source_text(delimiter: &MacroDelimiter, has_id: bool) -> Option<String> {
    let MacroDelimiter::Paren(paren) = delimiter else {
        return None;
    };
    let group = paren.span.join().source_text()?;
    let inner = group.strip_prefix('(')?.strip_suffix(')')?.trim();

    let expr = if has_id {
        strip_id_prefix(inner)?
    } else {
        inner
    };

    Some(expr.trim().to_owned())
}

/// Removes a leading `id = "<literal>",` from already validated attribute text.
fn strip_id_prefix(text: &str) -> Option<&str> {
    let mut escaped = false;
    let open = text.find('"')?;

    for (offset, ch) in text[open + 1..].char_indices() {
        match ch {
            '\\' if !escaped => escaped = true,

            '"' if !escaped => {
                let rest = text[open + 1 + offset + 1..].trim_start();

                return rest.strip_prefix(',');
            }

            _ => escaped = false,
        }
    }

    None
}

/// The marker a component macro leaves in place of a consumed `#[when]`, so the attribute
/// macro still runs (keeping a `use ..::when` import used) and passes the item through.
pub(super) fn lowered_marker() -> TokenStream {
    quote::quote!(@lowered)
}

/// Whether `tokens` is the [`lowered_marker`].
pub(super) fn is_lowered_marker(tokens: &TokenStream) -> bool {
    tokens.to_string() == lowered_marker().to_string()
}
