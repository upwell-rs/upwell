//! The arguments of `#[scope(..)]`.

use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{LitInt, LitStr, Token};

use crate::paths::Paths;

/// The arguments of `#[scope(parents = [..], generations = .., rank = .., id = "..", name = "..")]`.
pub struct ScopeArgs {
    pub(super) parents: Vec<syn::Path>,
    pub(super) generations: Option<Generations>,
    pub(super) rank: Option<LitInt>,
    pub(super) id: Option<LitStr>,
    pub(super) name: Option<LitStr>,
    upwell: Option<syn::Path>,
}

/// The generation policy `generations = ..` names.
pub(super) enum Generations {
    /// A policy keyword, `pinned` or `durable`, spanned at the keyword.
    Keyword(syn::Ident),
    /// A path to a `ScopeGenerations` value, emitted verbatim.
    Path(syn::Path),
}

impl ScopeArgs {
    pub(crate) fn paths(&self, default: Paths) -> Paths {
        default.resolve(self.upwell.clone(), None)
    }
}

impl Parse for ScopeArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut parents = None;
        let mut generations = None;
        let mut rank = None;
        let mut id = None;
        let mut name = None;
        let mut upwell = None;

        while !input.is_empty() {
            let key = syn::Ident::parse_any(input)?;

            match key.to_string().as_str() {
                "parents" => parents = Some(parse_parents(&key, input)?),
                "generations" => {
                    input.parse::<Token![=]>()?;
                    generations = Some(parse_generations(input.parse()?)?);
                }
                "rank" => {
                    input.parse::<Token![=]>()?;
                    rank = Some(parse_rank(input.parse()?)?);
                }
                "id" => {
                    input.parse::<Token![=]>()?;
                    id = Some(validated_id(input.parse()?)?);
                }
                "name" => {
                    input.parse::<Token![=]>()?;
                    name = Some(input.parse()?);
                }
                "upwell" => upwell = Some(crate::attr::parse_path_override(input)?),
                _ => {
                    return Err(syn::Error::new_spanned(
                        key,
                        "unknown argument; expected one of: `parents`, `generations`, `rank`, \
                         `id`, `name`, `upwell`",
                    ));
                }
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        let parents = parents.ok_or_else(|| {
            input.error(
                "#[scope] needs every parent it opens under, as `#[scope(parents = [Root])]`",
            )
        })?;

        Ok(Self {
            parents,
            generations,
            rank,
            id,
            name,
            upwell,
        })
    }
}

/// `= [Parent, ..]`, rejecting an empty list at `key`.
fn parse_parents(key: &syn::Ident, input: ParseStream) -> syn::Result<Vec<syn::Path>> {
    let content;

    input.parse::<Token![=]>()?;
    syn::bracketed!(content in input);

    let parents: Vec<syn::Path> = Punctuated::<syn::Path, Token![,]>::parse_terminated(&content)?
        .into_iter()
        .collect();

    if parents.is_empty() {
        return Err(syn::Error::new_spanned(
            key,
            "#[scope] needs at least one parent; `Root` opens the scope under the application root",
        ));
    }

    Ok(parents)
}

/// `pinned`, `durable`, or a path to a policy value.
fn parse_generations(path: syn::Path) -> syn::Result<Generations> {
    let Some(ident) = path.get_ident() else {
        return Ok(Generations::Path(path));
    };

    match ident.to_string().as_str() {
        "pinned" | "durable" => Ok(Generations::Keyword(ident.clone())),
        _ => Err(syn::Error::new_spanned(
            ident,
            "unknown generation policy; expected `pinned`, `durable`, or a path to a \
             `ScopeGenerations` value",
        )),
    }
}

/// A rank strictly between the transient scope (0) and the root (255).
fn parse_rank(rank: LitInt) -> syn::Result<LitInt> {
    match rank.base10_parse::<u8>() {
        Ok(1..=254) => Ok(rank),
        _ => Err(syn::Error::new_spanned(
            &rank,
            "a scope's rank must be between 1 and 254: the transient scope ranks 0 and the \
             application root 255",
        )),
    }
}

/// Checks an explicit `id = ".."` against the namespaced-identifier grammar, so a malformed
/// identity points at the literal instead of failing const evaluation.
fn validated_id(id: LitStr) -> syn::Result<LitStr> {
    let value = id.value();
    let segments = value.split('/').collect::<Vec<_>>();

    if segments.len() < 2
        || !segments
            .iter()
            .all(|segment| crate::entry::is_segment(segment))
    {
        return Err(syn::Error::new_spanned(
            &id,
            format!(
                "`{value}` is not a valid scope ID: it needs at least two `/`-separated \
                 segments of lowercase ASCII letters, digits, `.`, `_` or `-`, each starting \
                 with a letter or digit"
            ),
        ));
    }

    Ok(id)
}
