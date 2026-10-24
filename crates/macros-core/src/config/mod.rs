//! `#[config]` — implements the `ConfigProperties` trait for a config type and,
//! when given `#[config(path = "..")]`, auto-registers a binding into the
//! `CONFIG_BINDINGS` slice so `auto_discover` picks it up.
//!
//! `NAME` defaults to the type name (override with `#[config(name = "..")]`). The
//! type must also be `Deserialize`. Omitting `path` leaves binding to an explicit
//! `AppBuilder::config::<T>(path)` call — needed when the same type binds at
//! several paths.
//!
//! Applies to a `struct` or an `enum` (serde handles either). Fields may carry a
//! field-level `#[default = ".."]` whose value is a template string merged under the
//! config before deserialization, so a missing field falls back to a (possibly
//! templated) default that resolves through the normal `${..}` pipeline. On an enum, a
//! variant may be marked with a bare `#[default]` (or the cfg-gated
//! `#[cfg_attr(PRED, default)]`) to select it when the config names no variant.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    DeriveInput, Ident, LitStr, Token,
    parse::{Parse, ParseStream},
};

use crate::paths::Paths;

use defaults::build_defaults;

mod defaults;
mod schema;
mod serde_attrs;

/// Arguments of the `#[config(...)]` attribute on a config type.
#[derive(Default)]
pub struct ConfigArgs {
    name: Option<LitStr>,
    path: Option<LitStr>,
    /// Override for the core `upwell` facade root (`upwell = ::path`).
    upwell: Option<syn::Path>,
    /// Override for the plugin own-types root (`crate = ::path`).
    krate: Option<syn::Path>,
}

impl ConfigArgs {
    /// Resolves the crate [`Paths`] for this invocation: the macro's `default` roots with any
    /// `upwell =` / `crate =` overrides applied.
    pub fn paths(&self, default: Paths) -> Paths {
        default.resolve(self.upwell.clone(), self.krate.clone())
    }
}

impl Parse for ConfigArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = ConfigArgs::default();

        while !input.is_empty() {
            let key: Ident = input.parse()?;

            match key.to_string().as_str() {
                "name" => {
                    input.parse::<Token![=]>()?;
                    args.name = Some(input.parse()?);
                }
                "path" => {
                    input.parse::<Token![=]>()?;
                    args.path = Some(input.parse()?);
                }
                "upwell" => args.upwell = Some(crate::attr::parse_path_override(input)?),
                "crate" => args.krate = Some(crate::attr::parse_path_override(input)?),
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "unknown argument `{other}`, expected `name`, `path`, `upwell`, or `crate`"
                        ),
                    ));
                }
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(args)
    }
}

pub fn expand(args: ConfigArgs, mut item: DeriveInput, paths: &Paths) -> syn::Result<TokenStream> {
    let ident = item.ident.clone();

    let name = args
        .name
        .unwrap_or_else(|| LitStr::new(&ident.to_string(), ident.span()));
    let config_properties = paths.core("ConfigProperties");
    let config_binding_descriptor = paths.core("ConfigBindingDescriptor");
    let config_bindings = paths.core("CONFIG_BINDINGS");
    let descriptor = paths.core("Descriptor");
    let distributed_slice = paths.core("linkme::distributed_slice");
    let linkme_crate = paths.core("linkme");

    // Collect (and strip) the field-level `#[default = ".."]` attributes, then build the
    // `const DEFAULTS`. Stripping is required so the sibling `#[derive(Deserialize)]`
    // does not see the unknown attribute.
    let schema_const = schema::build_schema(&item, paths)?;
    let described_value = schema::described_value_impl(&item, paths);
    let config_value_schema = paths.core("schema::config_value_schema");
    let defaults_const = build_defaults(&mut item, paths)?;

    // A baked-in path exposes the binding on the type as `Descriptor<ConfigBindingDescriptor>`
    // and auto-registers it into the `CONFIG_BINDINGS` slice (picked up by
    // `ConfigManager::auto_discover`). Without a path the binding is made explicitly at the
    // manager/builder (the multi-path case), so no descriptor is emitted.
    let registration = match args.path {
        Some(path) => {
            let declared_ty = crate::source::declared_type(&ident, &ident, &name, paths);
            let condition_input = crate::when::config_input(&ident, &path, paths);

            quote! {
            impl #descriptor<#config_binding_descriptor> for #ident {
                const DESCRIPTOR: #config_binding_descriptor = #config_binding_descriptor {
                    ty: #declared_ty,
                    path: #path,
                    bind: <#ident as #config_properties>::bind,
                    slot: <#ident as #config_properties>::slot,
                    defaults: <#ident as #config_properties>::DEFAULTS,
                    describe: #config_value_schema::<#ident>,
                };
            }

            const _: () = {
                #[#distributed_slice(#config_bindings)]
                #[linkme(crate = #linkme_crate)]
                #[allow(unsafe_code)]
                static __UPWELL_CONFIG_BINDING: #config_binding_descriptor =
                    <#ident as #descriptor<#config_binding_descriptor>>::DESCRIPTOR;
            };

            #condition_input
            }
        }

        None => quote!(),
    };

    // The `ConfigProperties` impl and the `CONFIG_BINDINGS` (`linkme`) registration are the
    // server-side config machinery — gated out on wasm. The struct itself stays as a plain serde
    // DTO (harmless, and usable as shared data if a client references it).
    let server = crate::gate::native_only(quote! {
        impl #config_properties for #ident {
            const NAME: &'static str = #name;

            #defaults_const

            #schema_const
        }

        #described_value

        #registration
    });

    Ok(quote! {
        // The user's config type is kept on every target (usable as a plain serde DTO on a wasm
        // client); only the `ConfigProperties`/`CONFIG_BINDINGS` wiring above is server-gated.
        #[cfg_attr(target_family = "wasm", allow(dead_code))]
        #item

        #server
    })
}
