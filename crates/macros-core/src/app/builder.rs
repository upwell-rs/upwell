use proc_macro2::TokenStream;
use quote::quote;

use super::model::{AppAssembly, ConfigSettings, DirSettings, ManagerSource, ManagerValue};
use crate::{di, paths::Paths};

/// Expands the builder `assembly` declares, using paths already resolved by a named host.
pub(super) fn expand_with_paths(assembly: &AppAssembly, paths: &Paths) -> TokenStream {
    let name = &assembly.name.value;
    let protocol = &assembly.protocol.value;
    let components = &assembly.components;
    let middleware = &assembly.middleware;
    let guards = &assembly.guards;
    let config_tys = assembly.configs.iter().map(|entry| &entry.ty);
    let config_paths = assembly.configs.iter().map(|entry| &entry.path);
    let app_ty = paths.core("App");
    let assertion = expand_service_assertion(&assembly.services, paths);
    let (directories_binding, directories_call, directories_available) =
        match expand_directories(&assembly.directories_manager, paths) {
            Ok(expansion) => expansion,
            Err(error) => return error,
        };
    let (config_binding, config_call) =
        match expand_config(&assembly.config_manager, directories_available, paths) {
            Ok(expansion) => expansion,
            Err(error) => return error,
        };
    let deny_unplanned = assembly.deny_unplanned.then(|| quote!(.deny_unplanned()));
    let error_handler = assembly.error_handler.iter();

    quote! {
        {
            #assertion

            #directories_binding
            #config_binding

            #app_ty::<#protocol>::builder(#name)
                .auto_discover()
                #deny_unplanned
                #(.with_component(#components))*
                #(.config::<#config_tys>(#config_paths))*
                #config_call
                #directories_call
                #(.middleware(#middleware))*
                #(.guard(#guards))*
                #(.error_handler(#error_handler))*
        }
    }
}

fn expand_service_assertion(services: &[syn::Type], paths: &Paths) -> TokenStream {
    if di::enabled() && !services.is_empty() {
        let wired = paths.core("Wired");

        return quote! {
            const _: () = {
                fn __upwell_assert_wired<T: #wired>() {}

                fn __upwell_app_check() {
                    #(__upwell_assert_wired::<#services>();)*
                }
            };
        };
    }

    TokenStream::new()
}

fn expand_directories(
    manager: &Option<ManagerSource<DirSettings>>,
    paths: &Paths,
) -> Result<(TokenStream, TokenStream, bool), TokenStream> {
    let directories_path = paths.core("DirectoriesManager");

    match manager {
        Some(ManagerSource {
            value: ManagerValue::Instance(expression),
            ..
        }) => Ok((
            quote!(let __upwell_directories = #expression;),
            quote!(.directories(__upwell_directories)),
            true,
        )),
        Some(ManagerSource {
            value:
                ManagerValue::Configure {
                    block_span,
                    settings,
                },
            ..
        }) => {
            let expression = if let Some(root) = &settings.root {
                let root = &root.value;

                quote!(#directories_path::from_path(#root))
            } else if let Some(app) = &settings.app {
                let app = &app.value;

                quote!(#directories_path::for_app(#app))
            } else {
                return Err(error(
                    *block_span,
                    "a `directories` config block needs `app` or `root`",
                ));
            };

            Ok((
                quote!(let __upwell_directories = #expression;),
                quote!(.directories(__upwell_directories)),
                true,
            ))
        }
        None => Ok((TokenStream::new(), TokenStream::new(), false)),
    }
}

fn expand_config(
    manager: &Option<ManagerSource<ConfigSettings>>,
    directories_available: bool,
    paths: &Paths,
) -> Result<(TokenStream, TokenStream), TokenStream> {
    let config_manager_path = paths.core("ConfigManager");
    let config_dynamic = paths.core("config::Dynamic");

    match manager {
        Some(ManagerSource {
            value: ManagerValue::Instance(expression),
            ..
        }) => Ok((
            quote!(let __upwell_config = #expression;),
            quote!(.config_source(__upwell_config)),
        )),
        Some(ManagerSource {
            key_span,
            value: ManagerValue::Configure { settings, .. },
        }) => {
            let base = if let Some(source) = &settings.source {
                let source = &source.value;

                quote!(#source)
            } else if directories_available {
                let profiles = match &settings.profiles {
                    Some(profiles) => {
                        let profiles = &profiles.value;

                        quote!(#profiles)
                    }
                    None => quote!(&[]),
                };

                quote!(#config_manager_path::<#config_dynamic>::load_from(&__upwell_directories, #profiles)?)
            } else {
                return Err(error(
                    *key_span,
                    "a `config` block without `source` requires a `directories` manager to load from",
                ));
            };
            let mut chain = base;

            if settings
                .sighup
                .as_ref()
                .is_some_and(|setting| setting.value)
            {
                chain = quote!(#chain.reload_on_sighup());
            }

            if settings.watch.as_ref().is_some_and(|setting| setting.value) {
                chain = quote!(#chain.watch_config());
            }

            if let Some(debounce) = &settings.debounce {
                let debounce = &debounce.value;

                chain = quote!(#chain.config_reload_debounce(#debounce));
            }

            Ok((
                quote!(let __upwell_config = #chain;),
                quote!(.config_source(__upwell_config)),
            ))
        }
        None => Ok((TokenStream::new(), TokenStream::new())),
    }
}

fn error(span: proc_macro2::Span, message: &str) -> TokenStream {
    syn::Error::new(span, message).to_compile_error()
}
