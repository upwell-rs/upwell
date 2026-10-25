//! The RPC service extension: `Router`, the [`ComponentExt`] that turns `#[component]` into
//! `#[service]`.
//!
//! `#[service]` is `ComponentArgs<Router>` (aliased [`RouterComponent`]). The base component
//! macro emits the singleton component (field-injection factory, `Component` impl, providers);
//! `Router` appends the **router** surface: a `ServiceComponent` impl (the version), the
//! service's `{Service}Rpcs` slice + `ServiceRpcs` impl (where `#[handlers]` blocks register
//! their RPCs), the `ServiceDescriptor` in the `SERVICES` slice, and the generated client
//! struct. It captures the base-resolved identity via [`ParseItem<ComponentContext>`] so the
//! service descriptor's id/name match the component.
//!
//! `Router<T>` nests a further extension `T` (default [`NoExt`]) so a richer router macro —
//! a future `#[controller]` — can layer its own keys and emission on top.

use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::parse::ParseStream;
use syn::{Ident, LitStr};

use upwell_macros_core::attr::ComponentArgs;
use upwell_macros_core::paths::Paths;
use upwell_macros_core::{ComponentContext, ComponentExt, NoExt, ParseItem, ParseKeyed, eat_eq};

/// The `#[service]` args: the base component args extended with the RPC [`Router`]. (The
/// `T: ComponentExt` bound is enforced through `Router<T>`'s own impls, not the alias.)
pub type RouterComponent<T = NoExt> = ComponentArgs<Router<T>>;

/// The RPC service extension. Adds `version` / `rpc_slice` keyed args, captures the component
/// identity, and emits the service header + RPC slice + client. Nests `T` for further
/// extension.
#[derive(Default)]
pub struct Router<T: ComponentExt = NoExt> {
    /// `version = ".."` — the service version carried on `ServiceComponent`.
    version: Option<LitStr>,
    /// `rpc_slice = Ident` — the per-service RPC slice name (default `{Service}Rpcs`).
    rpc_slice: Option<Ident>,
    /// The base-resolved component identity (captured in the item pass).
    context: Option<RouterContext>,
    /// The nested extension.
    inner: T,
}

/// The component identity (and resolved crate paths) `Router` keeps to emit the service surface.
struct RouterContext {
    ident: Ident,
    type_name: LitStr,
    id: LitStr,
    name: LitStr,
    paths: Paths,
}

impl<T: ComponentExt> ParseKeyed for Router<T> {
    fn parse_keyed(&mut self, key: &Ident, input: ParseStream) -> syn::Result<bool> {
        match key.to_string().as_str() {
            "version" => {
                eat_eq(input)?;
                self.version = Some(input.parse()?);

                Ok(true)
            }

            "rpc_slice" => {
                eat_eq(input)?;
                self.rpc_slice = Some(input.parse()?);

                Ok(true)
            }

            // Unknown to the router — offer it to the nested extension.
            _ => self.inner.parse_keyed(key, input),
        }
    }

    fn expected_keys() -> &'static [&'static str] {
        &["version", "rpc_slice"]
    }
}

impl<T: ComponentExt> ParseItem<ComponentContext> for Router<T> {
    fn parse_item(&mut self, cx: &ComponentContext, paths: &Paths) -> syn::Result<()> {
        // A service is always a singleton; reject a non-singleton scope on the component.
        if let Some(scope) = &cx.scope
            && scope
                .segments
                .last()
                .is_none_or(|seg| seg.ident != "Singleton")
        {
            return Err(syn::Error::new_spanned(
                scope,
                "#[service] components are always singletons; `scope` is only valid on #[component]",
            ));
        }

        upwell_macros_core::entry::owner_id(&cx.id)?;

        self.context = Some(RouterContext {
            ident: cx.ident.clone(),
            type_name: cx.type_name.clone(),
            id: cx.id.clone(),
            name: cx.name.clone(),
            paths: paths.clone(),
        });

        // Forward to the nested extension, if it also wants the identity.
        self.inner.parse_item(cx, paths)
    }
}

impl<T: ComponentExt> ComponentExt for Router<T> {
    fn defers_factory(&self) -> bool {
        // A service's factory may be overridden by an `#[init]` in a `#[handlers]` impl, so the
        // base defers its eager field-DI assertion.
        true
    }

    fn asserts_wired(&self) -> bool {
        // A service is a router-class component: force its `Wired` graph check at its own
        // definition, so a missing provider is caught there rather than at an `app!` listing.
        true
    }
}

impl<T: ComponentExt> ToTokens for Router<T> {
    fn to_tokens(&self, out: &mut TokenStream) {
        let Some(cx) = &self.context else {
            return;
        };

        let RouterContext {
            ident,
            type_name,
            id,
            name,
            paths,
        } = cx;
        let version = match &self.version {
            Some(v) => quote!(::core::option::Option::Some(#v)),
            None => quote!(::core::option::Option::None),
        };

        let client_struct = client_struct(ident);

        let service_static = format_ident!("__UPWELL_SERVICE_{}", ident.to_string().to_uppercase());
        let rpcs_slice = self
            .rpc_slice
            .clone()
            .unwrap_or_else(|| format_ident!("{}Rpcs", ident));
        let descriptor_trait = paths.core("Descriptor");
        let distributed_slice = paths.core("linkme::distributed_slice");
        let linkme_crate = paths.core("linkme");
        let inventory = paths.core("inventory");
        let descriptor_for = paths.core("DescriptorFor");
        let rpc_group = paths.plugin("RpcGroup");
        let service_component = paths.core("ServiceComponent");
        let service_descriptor = paths.plugin("ServiceDescriptor");
        let service_rpcs = paths.plugin("ServiceRpcs");
        let services_slice = paths.plugin("SERVICES");
        let declared_ty = upwell_macros_core::source::declared_type(ident, ident, type_name, paths);
        let inner = &self.inner;

        // The service's own RPC groups — a single-kind per-type registry (no `Registration` merge),
        // so the `linkme` accessor returns its slice directly while the `inventory` accessor
        // materializes its bucket into a `OnceLock` cache. Both return `&'static [RpcGroup]`.
        let registry = upwell_macros_core::backend::registry_for_impl(
            quote!(#ident),
            quote!(#rpc_group),
            paths,
        );
        let rpcs_infra = upwell_macros_core::backend::dual_backend(
            quote! {
                #registry

                impl #service_rpcs for #ident {
                    fn rpc_groups() -> &'static [#rpc_group] {
                        static CACHE: ::std::sync::OnceLock<::std::vec::Vec<#rpc_group>> =
                            ::std::sync::OnceLock::new();

                        CACHE
                            .get_or_init(|| {
                                #inventory::iter::<#descriptor_for<#ident, #rpc_group>>
                                    .into_iter()
                                    .map(|__entry| **__entry)
                                    .collect()
                            })
                            .as_slice()
                    }
                }
            },
            quote! {
                #[#distributed_slice]
                #[linkme(crate = #linkme_crate)]
                #[allow(non_upper_case_globals)]
                pub static #rpcs_slice: [#rpc_group];

                impl #service_rpcs for #ident {
                    fn rpc_groups() -> &'static [#rpc_group] {
                        &#rpcs_slice
                    }
                }
            },
        );

        out.extend(quote! {
            #client_struct

            impl #service_component for #ident {
                const VERSION: ::core::option::Option<&'static str> = #version;
            }

            #rpcs_infra

            const _: () = {
                const __UPWELL_SERVICE_DESCRIPTOR: #service_descriptor =
                    #service_descriptor {
                        id: #id,
                        name: #name,
                        ty: #declared_ty,
                        version: #version,
                        rpcs: <#ident as #service_rpcs>::rpc_groups,
                    };

                impl #descriptor_trait<#service_descriptor> for #ident {
                    const DESCRIPTOR: #service_descriptor = __UPWELL_SERVICE_DESCRIPTOR;
                }

                #[#distributed_slice(#services_slice)]
                #[linkme(crate = #linkme_crate)]
                static #service_static: #service_descriptor = __UPWELL_SERVICE_DESCRIPTOR;
            };

            #inner
        });
    }
}

/// The per-service client struct (its methods are contributed by `#[handlers]` blocks as
/// capability-partitioned impls). Emitted once per service, gated on the `client` feature.
fn client_struct(ident: &Ident) -> TokenStream {
    if !cfg!(feature = "client") {
        return quote!();
    }

    let client_ident = format_ident!("{}Client", ident);

    quote! {
        /// Generated RPC client: wraps a transport `C` and exposes one method per `#[rpc]`,
        /// each available only when `C` supports that call shape's capability.
        pub struct #client_ident<C>(pub C);

        impl<C> #client_ident<C> {
            /// Wraps a protocol transport (e.g. `StreamClientTransport`).
            pub fn new(transport: C) -> Self {
                Self(transport)
            }
        }
    }
}
