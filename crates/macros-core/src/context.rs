//! The `#[context]` macro: a factory-less, by-value value a protocol or the application seeds
//! into an instance of each scope it lists.
//!
//! It lowers to the component expansion with the scope list, `by_value`, no default factory
//! and the `Context` construction policy, so placements, discovery and `di-check` work as
//! for any component. Only the policy differs: it lets the value be seeded in several scopes
//! that share no hierarchy, and makes preparation check that a seed claim covers each
//! placement something depends on.

use syn::parse::{Parse, ParseStream};
use syn::{Ident, ItemStruct, LitStr, Token};

use crate::attr::{ComponentArgs, parse_path_override};
use crate::paths::Paths;
use crate::scopes::ScopeList;

/// The keys `#[context]` accepts, for the "unknown argument" diagnostic.
const CONTEXT_KEYS: &[&str] = &["scopes", "id", "name", "upwell"];

/// The arguments of `#[context(scopes = [..], id = "..", name = "..")]`.
pub struct ContextArgs {
    /// The `scopes` key, kept for its span: the scope checks point at it.
    keyword: Ident,
    scopes: Option<ScopeList>,
    id: Option<LitStr>,
    name: Option<LitStr>,
    upwell: Option<syn::Path>,
}

impl Parse for ContextArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = Self {
            keyword: Ident::new("context", input.span()),
            scopes: None,
            id: None,
            name: None,
            upwell: None,
        };

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            let repeated = match key.to_string().as_str() {
                "scopes" => {
                    input.parse::<Token![=]>()?;
                    args.keyword = key.clone();
                    args.scopes.replace(input.parse()?).is_some()
                }
                "id" => {
                    input.parse::<Token![=]>()?;
                    args.id.replace(input.parse()?).is_some()
                }
                "name" => {
                    input.parse::<Token![=]>()?;
                    args.name.replace(input.parse()?).is_some()
                }
                "upwell" => args.upwell.replace(parse_path_override(input)?).is_some(),
                other => {
                    return Err(syn::Error::new_spanned(
                        &key,
                        format!(
                            "unknown `#[context]` argument `{other}`, expected one of: {}",
                            CONTEXT_KEYS.join(", ")
                        ),
                    ));
                }
            };

            if repeated {
                return Err(syn::Error::new_spanned(
                    &key,
                    format!("`{key}` is given twice"),
                ));
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(args)
    }
}

impl ContextArgs {
    /// The component arguments the context lowers to, or an error at the attribute when it
    /// names no scope.
    fn into_component(self, item: &ItemStruct) -> syn::Result<ComponentArgs> {
        let Some(scopes) = self.scopes else {
            return Err(syn::Error::new_spanned(
                &item.ident,
                "`#[context]` needs `scopes = [..]`: the scopes a protocol or the application \
                 seeds the value into, as in `#[context(scopes = [WebsocketConnection, HttpRequest])]`",
            ));
        };

        let outside = scopes.paths().iter().find(|path| {
            path.segments.last().is_some_and(|segment| {
                ["Singleton", "Root", "Transient"]
                    .iter()
                    .any(|name| segment.ident == name)
            })
        });

        if let Some(path) = outside {
            return Err(syn::Error::new_spanned(
                path,
                "a context value lives only in scopes below the root: it is seeded into scope \
                 instances, and the root and the transient scope open none",
            ));
        }

        Ok(ComponentArgs {
            id: self.id,
            name: self.name,
            by_value: true,
            scope: Some(scopes),
            no_default_factory: true,
            context: Some(self.keyword),
            upwell: self.upwell,
            ..ComponentArgs::default()
        })
    }
}

/// Expands `#[context]` on `item` through the component expansion.
pub fn expand(
    args: ContextArgs,
    item: ItemStruct,
    default: Paths,
) -> syn::Result<proc_macro2::TokenStream> {
    let args = args.into_component(&item)?;
    let paths = args.paths(default);

    crate::component::expand(args, item, &paths)
}

#[cfg(test)]
mod tests;
