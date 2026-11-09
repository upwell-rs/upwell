//! The `scope = ..` argument shared by `#[component]` and `#[init]`: one scope marker path,
//! or a bracketed list of the disjoint scopes one component type builds in.
//!
//! A list places the component (or the factory) in each scope it names, one placement per
//! scope. The scopes of one type may not share a hierarchy, which preparation checks against
//! the scope topology; the macro rejects only what it can see: an empty list and a scope
//! listed twice.

use proc_macro2::TokenStream;
use quote::{ToTokens, quote, quote_spanned};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Token, token};

use crate::attr::scope_alias;
use crate::paths::Paths;

/// The scopes a `scope = ..` argument names: `scope = Request` or
/// `scope = [WebsocketConnection, HttpRequest]`, each path emitted as written so it resolves
/// in the caller's scope (the lowercase aliases aside). Its tokens are the argument as
/// written, so a diagnostic about the whole argument points at it.
#[derive(Clone)]
pub struct ScopeList {
    paths: Vec<syn::Path>,
    written: TokenStream,
}

impl ScopeList {
    /// The declared scope: the only path, or the first of a list.
    pub fn primary(&self) -> &syn::Path {
        &self.paths[0]
    }

    /// Every listed path, in declaration order.
    pub fn paths(&self) -> &[syn::Path] {
        &self.paths
    }

    /// Whether the argument names more than one scope.
    pub fn is_several(&self) -> bool {
        self.paths.len() > 1
    }

    /// The `&[<A as Scope>::KEY, ..]` slice of every listed scope, each key spanned at its
    /// path so a type that is not a scope is reported where it is written.
    pub fn keys(&self, paths: &Paths) -> TokenStream {
        let scope_trait = paths.core("Scope");
        let keys = self
            .paths
            .iter()
            .map(|path| quote_spanned! {path.span()=> <#path as #scope_trait>::KEY });

        quote!(&[#(#keys),*])
    }

    /// The descriptor's `scopes` slice: every key when the argument names several scopes,
    /// and empty for one, which the descriptor's single scope already carries.
    pub fn declared_keys(&self, paths: &Paths) -> TokenStream {
        match self.is_several() {
            true => self.keys(paths),
            false => quote!(&[]),
        }
    }
}

impl Parse for ScopeList {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut written = TokenStream::new();

        if !input.peek(token::Bracket) {
            let path: syn::Path = input.parse()?;

            path.to_tokens(&mut written);

            return Ok(Self {
                paths: vec![scope_alias(path)],
                written,
            });
        }

        let content;
        let bracket = syn::bracketed!(content in input);
        let listed = Punctuated::<syn::Path, Token![,]>::parse_terminated(&content)?;

        bracket.surround(&mut written, |inner| listed.to_tokens(inner));

        if listed.is_empty() {
            return Err(syn::Error::new(
                bracket.span.join(),
                "a scope list needs at least one scope, as in \
                 `scope = [WebsocketConnection, HttpRequest]`",
            ));
        }

        Ok(Self {
            paths: distinct(listed)?,
            written,
        })
    }
}

impl ToTokens for ScopeList {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        self.written.to_tokens(tokens);
    }
}

/// The listed paths with their aliases resolved, rejecting a scope listed twice at its second
/// mention. Paths compare by their tokens after alias resolution, so `request` and `Request`
/// are one scope; one type named through two different paths is caught at preparation.
fn distinct(listed: Punctuated<syn::Path, Token![,]>) -> syn::Result<Vec<syn::Path>> {
    let mut paths: Vec<syn::Path> = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    for written in listed {
        let path = scope_alias(written.clone());
        let key = path.to_token_stream().to_string();

        if seen.contains(&key) {
            return Err(syn::Error::new_spanned(
                written,
                format!("`{key}` is listed twice; a component builds once in each scope"),
            ));
        }

        seen.push(key);
        paths.push(path);
    }

    Ok(paths)
}

#[cfg(test)]
mod tests;
