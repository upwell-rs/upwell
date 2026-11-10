//! Parameter attributes of an `#[init]` constructor: `#[config("path")]` and
//! `#[qualifier = ".."]`, spelled as on a `#[component]` field.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, Expr, ExprLit, Lit, LitStr, Meta, PatType, Type};

use crate::paths::Paths;

/// How an `#[init]` parameter is bound beyond its type.
pub(super) enum ParamBinding {
    /// Resolved by its type alone.
    Plain,
    /// A `Cfg<T>` bound at this property path.
    Config(LitStr),
    /// An injectable handle selecting the provider with this qualifier.
    Qualifier(LitStr),
}

/// One `#[init]` parameter: its type and binding.
pub(super) struct InitParam {
    pub(super) ty: Type,
    pub(super) binding: ParamBinding,
}

impl InitParam {
    /// Reads and strips the binding attributes of `arg`.
    pub(super) fn take(arg: &mut PatType) -> syn::Result<Self> {
        let mut binding = ParamBinding::Plain;
        let mut kept = Vec::new();

        for attr in arg.attrs.drain(..) {
            let next = match attr_binding(&attr)? {
                Some(next) => next,
                None => {
                    kept.push(attr);

                    continue;
                }
            };

            if !matches!(binding, ParamBinding::Plain) && !matches!(next, ParamBinding::Plain) {
                return Err(syn::Error::new_spanned(
                    attr,
                    "an `#[init]` parameter takes at most one of `#[config(..)]` and `#[qualifier = ..]`",
                ));
            }

            if !matches!(next, ParamBinding::Plain) {
                binding = next;
            }
        }

        arg.attrs = kept;

        Ok(Self {
            ty: (*arg.ty).clone(),
            binding,
        })
    }

    /// Whether the parameter needs a generated wrapper type.
    pub(super) fn is_bound(&self) -> bool {
        !matches!(self.binding, ParamBinding::Plain)
    }

    /// The wrapper struct and `FromContainer` impl named `name` for this parameter, or
    /// nothing for a plain one.
    pub(super) fn wrapper(&self, name: &syn::Ident, paths: &Paths) -> TokenStream {
        let from_container = paths.core("FromContainer");
        let construction_context = paths.core("ComponentConstructionContext");
        let dependency_descriptor = paths.core("DependencyDescriptor");
        let result = paths.core("DiResult");
        let error = paths.core("DiError");
        let ty = &self.ty;
        let label = quote!(::core::stringify!(#ty));

        let (qualifier, resolve) = match &self.binding {
            ParamBinding::Plain => return quote!(),
            ParamBinding::Config(path) => {
                let config_store = paths.core("ConfigStore");
                let resolver_ctx_ext = paths.core("ResolverCtxExt");

                (
                    path,
                    quote! {
                        #resolver_ctx_ext::get_resolver::<#config_store>(cx)
                            .and_then(|store| store.resolve_path::<#ty>(#path))
                    },
                )
            }
            ParamBinding::Qualifier(qualifier) => (
                qualifier,
                quote!(cx.resolve_qualified::<#ty>(#qualifier).await?),
            ),
        };

        quote! {
            struct #name(#ty);

            impl #from_container for #name {
                fn dependency() -> #dependency_descriptor {
                    let mut dependency = <#ty as #from_container>::dependency();

                    dependency.qualifier = ::core::option::Option::Some(#qualifier);

                    dependency
                }

                async fn from_container(cx: &#construction_context) -> #result<Self> {
                    #resolve
                        .map(Self)
                        .ok_or(#error::MissingComponent(#label))
                }
            }
        }
    }
}

fn attr_binding(attr: &Attribute) -> syn::Result<Option<ParamBinding>> {
    if attr.path().is_ident("config") {
        return Ok(Some(match &attr.meta {
            Meta::Path(_) => ParamBinding::Plain,
            Meta::List(list) => ParamBinding::Config(syn::parse2::<LitStr>(list.tokens.clone())?),
            Meta::NameValue(_) => {
                return Err(syn::Error::new_spanned(
                    attr,
                    "`#[config]` on an `#[init]` parameter is `#[config]` or `#[config(\"path\")]`",
                ));
            }
        }));
    }

    if attr.path().is_ident("qualifier") {
        if let Meta::NameValue(nv) = &attr.meta
            && let Expr::Lit(ExprLit {
                lit: Lit::Str(value),
                ..
            }) = &nv.value
        {
            return Ok(Some(ParamBinding::Qualifier(value.clone())));
        }

        return Err(syn::Error::new_spanned(
            attr,
            "`#[qualifier]` on an `#[init]` parameter is `#[qualifier = \"name\"]`",
        ));
    }

    Ok(None)
}
