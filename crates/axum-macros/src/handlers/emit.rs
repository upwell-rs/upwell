use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::{LitStr, Path};

use super::AxumHandlers;

type RouteEntry<'a> = (&'a syn::Ident, &'a [Path], &'a TokenStream);

impl ToTokens for AxumHandlers {
    fn to_tokens(&self, out: &mut TokenStream) {
        let Some(cx) = &self.context else {
            return;
        };

        if !self.routes.is_empty() && !self.ws_routes.is_empty() {
            out.extend(quote! {
                ::core::compile_error!(
                    "a #[handlers] block mixes HTTP route attributes with #[message] handlers; \
                     split them into separate impl blocks"
                );
            });
            return;
        }
        if !self.ws_routes.is_empty() {
            super::ws_emit::emit(self, cx, out);
            return;
        }
        if self.routes.is_empty() {
            return;
        }
        emit_http(self, cx, out);
    }
}

fn emit_http(handlers: &AxumHandlers, cx: &super::HandlerContext, out: &mut TokenStream) {
    let paths = &cx.paths;
    let self_ty = &cx.self_ty;
    let axum = paths.plugin("axum");
    let app_runtime = paths.core("AppRuntime");
    let as_layer = paths.plugin("middleware::as_layer");
    let distributed_slice = paths.core("linkme::distributed_slice");
    let linkme_crate = paths.core("linkme");
    let inventory = paths.core("inventory");
    let descriptor_for = paths.core("DescriptorFor");
    let controller_route = paths.plugin("ControllerRoute");
    let http_route_descriptor = paths.plugin("HttpRouteDescriptor");
    let http_path_parameter_descriptor = paths.plugin("HttpPathParameterDescriptor");
    let http_input_descriptor = paths.plugin("HttpInputDescriptor");
    let http_input_source = paths.plugin("HttpInputSource");
    let http_output_descriptor = paths.plugin("HttpOutputDescriptor");
    let http_output_shape = paths.plugin("HttpOutputShape");
    let http_response_descriptor = paths.plugin("HttpResponseDescriptor");
    let http_response_body_descriptor = paths.plugin("HttpResponseBodyDescriptor");
    let type_descriptor = paths.core("TypeDescriptor");
    let descriptor_trait = paths.core("Descriptor");
    let from_container = paths.core("FromContainer");
    let entry_key = paths.core("entry_key");
    let controller_descriptor = paths.plugin("ControllerDescriptor");
    let controller_trait = paths.plugin("Controller");
    let routes_slice = handlers
        .routes_slice
        .clone()
        .unwrap_or_else(|| format_ident!("{}Routes", cx.self_ident));
    let mut groups: Vec<(LitStr, Vec<RouteEntry>)> = Vec::new();

    for spec in &handlers.routes {
        let value = spec.path.value();
        let entry = (&spec.verb, spec.middleware.as_slice(), &spec.handler);
        match groups.iter_mut().find(|(path, _)| path.value() == value) {
            Some((_, entries)) => entries.push(entry),
            None => groups.push((spec.path.clone(), vec![entry])),
        }
    }

    let layer_route = |base: TokenStream, middleware: &[Path]| -> TokenStream {
        let mut chain = base;
        for mw in middleware.iter().rev() {
            chain = quote! {
                #chain.layer(#as_layer(
                    runtime.root().get::<#mw>().expect(
                        "middleware component missing from DI root — did you register it?",
                    ),
                ))
            };
        }
        chain
    };
    let route_tokens = groups.iter().map(|(path, entries)| {
        let mut entries = entries.iter();
        let (first_verb, first_middleware, first_handler) =
            entries.next().expect("group has at least one route");
        let mut chain = layer_route(
            quote!(#axum::routing::#first_verb(#first_handler)),
            first_middleware,
        );
        for (verb, middleware, handler) in entries {
            let verb_router = layer_route(quote!(#axum::routing::#verb(#handler)), middleware);
            chain = quote!(#chain.merge(#verb_router));
        }
        quote!(.route(#path, #chain))
    });
    let register = upwell_macros_core::backend::dual_backend(
        quote! {
            #inventory::submit! {
                #descriptor_for::<#self_ty, #controller_route<#self_ty>>::new(
                    #controller_route {
                        build: __upwell_axum_route_group,
                        routes: __UPWELL_AXUM_ROUTE_DESCRIPTORS,
                    }
                )
            }
        },
        quote! {
            #[#distributed_slice(#routes_slice)]
            #[linkme(crate = #linkme_crate)]
            static __UPWELL_AXUM_ROUTE_GROUP: #controller_route<#self_ty> =
                #controller_route {
                    build: __upwell_axum_route_group,
                    routes: __UPWELL_AXUM_ROUTE_DESCRIPTORS,
                };
        },
    );
    let route_descriptors = handlers.routes.iter().map(|route| {
        let handler = route.handler_name.to_string();
        let source = upwell_macros_core::source::item_source(
            route.handler_name.span(),
            &upwell_macros_core::source::associated_item(self_ty, &route.handler_name),
            paths,
        );
        let method = route.verb.to_string().to_ascii_uppercase();
        let path = &route.path;
        let targets = route
            .inputs
            .iter()
            .filter(|input| input.source == "Injected")
            .map(|input| {
                let ty = &input.ty;

                quote!(<#ty as #from_container>::dependency())
            });
        let input_descriptors = route.inputs.iter().map(|input| {
            let name = &input.name;
            let source = format_ident!("{}", input.source);
            let ty = &input.ty;
            let ty_name = ty.to_token_stream().to_string();
            quote! {
                #http_input_descriptor {
                    name: #name,
                    source: #http_input_source::#source,
                    ty: #type_descriptor::of::<#ty>(#ty_name),
                }
            }
        });
        let path_parameters = route.path_parameters.iter().map(|(name, catch_all)| {
            quote! { #http_path_parameter_descriptor { name: #name, catch_all: #catch_all } }
        });
        let output_shape = format_ident!("{}", route.output.shape);
        let declared_name = route.output.declared.to_token_stream().to_string();
        let output_ty = route
            .output
            .ty
            .as_ref()
            .map(|ty| {
                let name = ty.to_token_stream().to_string();
                quote!(::core::option::Option::Some(#type_descriptor::of::<#ty>(#name)))
            })
            .unwrap_or_else(|| quote!(::core::option::Option::None));
        let responses = route.output.responses.alternatives.iter().map(|response| {
            let status = response.status;
            let body = match &response.body {
                crate::http_analysis::ResponseBody::Typed(ty) => {
                    let name = ty.to_token_stream().to_string();
                    quote!(#http_response_body_descriptor::Typed(#type_descriptor::of::<#ty>(#name)))
                }
                crate::http_analysis::ResponseBody::Empty => quote!(#http_response_body_descriptor::Empty),
                crate::http_analysis::ResponseBody::Opaque => quote!(#http_response_body_descriptor::Opaque),
            };
            let redirect = response
                .redirect
                .as_ref()
                .map(|target| quote!(::core::option::Option::Some(#target)))
                .unwrap_or_else(|| quote!(::core::option::Option::None));
            quote! { #http_response_descriptor { status: #status, body: #body, redirect: #redirect } }
        });
        quote! {
            #http_route_descriptor {
                handler: #handler,
                method: #method,
                path: #path,
                path_parameters: &[#(#path_parameters),*],
                inputs: &[#(#input_descriptors),*],
                output: #http_output_descriptor {
                    ty: #output_ty,
                    declared: #declared_name,
                    shape: #http_output_shape::#output_shape,
                    responses: &[#(#responses),*],
                },
                source: ::core::option::Option::Some(#source),
                key: #entry_key!(
                    "http/",
                    <#self_ty as #descriptor_trait<#controller_descriptor>>::DESCRIPTOR.id,
                    ".",
                    #handler,
                ),
                targets: || ::std::vec![#(#targets),*],
            }
        }
    });

    out.extend(quote! {
        const _: () = {
            fn __upwell_assert_controller<T: #controller_trait>() {}
            let _ = __upwell_assert_controller::<#self_ty>;

            static __UPWELL_AXUM_ROUTE_DESCRIPTORS: &[#http_route_descriptor] = &[
                #(#route_descriptors),*
            ];

            fn __upwell_axum_route_group(
                svc: ::std::sync::Arc<#self_ty>,
                runtime: & #app_runtime,
            ) -> #axum::Router {
                let _ = &svc;
                let _ = runtime;
                #axum::Router::new() #(#route_tokens)*
            }

            #register
        };
    });
    let openapi_ops = &handlers.openapi_ops;
    out.extend(quote! { #(#openapi_ops)* });
}
