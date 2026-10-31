use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};

use super::{AxumHandlers, HandlerContext};

pub(super) fn emit(handlers: &AxumHandlers, cx: &HandlerContext, out: &mut TokenStream) {
    let paths = &cx.paths;
    let self_ty = &cx.self_ty;
    let distributed_slice = paths.core("linkme::distributed_slice");
    let linkme_crate = paths.core("linkme");
    let inventory = paths.core("inventory");
    let descriptor_for = paths.core("DescriptorFor");
    let controller_ws_route = paths.plugin("ControllerWsRoute");
    let ws_controller_trait = paths.plugin("WebsocketController");
    let ws_route = paths.plugin("WsRoute");
    let ws_route_descriptor = paths.plugin("WsRouteDescriptor");
    let ws_message_descriptor = paths.plugin("WsMessageDescriptor");
    let ws_message_mode = paths.plugin("WsMessageMode");
    let type_descriptor = paths.core("TypeDescriptor");
    let entry_key_owner = paths.core("EntryKeyOwner");
    let from_container = paths.core("FromContainer");
    let entry_key = paths.core("entry_key");
    let ws_routes_slice = handlers
        .routes_slice
        .clone()
        .unwrap_or_else(|| format_ident!("{}WsRoutes", cx.self_ident));
    let protocol = handlers
        .ws_protocol
        .as_ref()
        .expect("message routes require a handlers protocol");
    let ws_route_p = quote!(#ws_route<#protocol>);
    let descriptors = handlers.ws_routes.iter().map(|spec| {
        let destination = &spec.destination;
        let builder = &spec.builder;
        let handler = spec.handler_name.to_string();
        let key_segment = &spec.key_segment;
        let source = upwell_macros_core::source::item_source(
            spec.handler_name.span(),
            &upwell_macros_core::source::associated_item(self_ty, &spec.handler_name),
            paths,
        );
        let payload = spec
            .payload
            .as_ref()
            .map(|ty| {
                let name = ty.to_token_stream().to_string();
                quote!(::core::option::Option::Some(#type_descriptor::of::<#ty>(#name)))
            })
            .unwrap_or_else(|| quote!(::core::option::Option::None));
        let mode = if spec.is_request {
            quote!(#ws_message_mode::Request)
        } else {
            quote!(#ws_message_mode::Send)
        };
        let reply = spec
            .reply
            .as_ref()
            .map(|ty| {
                let name = ty.to_token_stream().to_string();
                quote!(::core::option::Option::Some(#type_descriptor::of::<#ty>(#name)))
            })
            .unwrap_or_else(|| quote!(::core::option::Option::None));
        let codec = &spec.codec;
        let codec_name = codec.to_string();
        let targets = spec
            .targets
            .iter()
            .map(|ty| quote!(<#ty as #from_container>::dependency()));
        quote! {
            #ws_route_descriptor::new_described::<#protocol>(
                #ws_message_descriptor {
                    handler: #handler,
                    destination: #destination,
                    payload: #payload,
                    mode: #mode,
                    reply: #reply,
                    codec: #type_descriptor::of::<#codec>(#codec_name),
                    source: ::core::option::Option::Some(#source),
                    key: #entry_key!(
                        "ws/",
                        <#self_ty as #entry_key_owner>::KEY_SEGMENT,
                        ".",
                        #key_segment,
                    ),
                    targets: || ::std::vec![#(#targets),*],
                },
                |runtime| {
                    let svc = runtime
                        .root()
                        .get::<#self_ty>()
                        .expect("ws controller singleton missing from the root scope");
                    let route: #ws_route_p = #builder;
                    route.handler
                },
            )
        }
    });
    let register = upwell_macros_core::backend::dual_backend(
        quote! {
            #inventory::submit! {
                #descriptor_for::<#self_ty, #controller_ws_route<#self_ty, #protocol>>::new(
                    #controller_ws_route::new(__upwell_ws_route_group)
                )
            }
        },
        quote! {
            #[#distributed_slice(#ws_routes_slice)]
            #[linkme(crate = #linkme_crate)]
            static __UPWELL_WS_ROUTE_GROUP: #controller_ws_route<#self_ty, #protocol> =
                #controller_ws_route::new(__upwell_ws_route_group);
        },
    );

    out.extend(quote! {
        const _: () = {
            fn __upwell_assert_ws_controller<
                T: #ws_controller_trait<Protocol = #protocol>,
            >() {}
            let _ = __upwell_assert_ws_controller::<#self_ty>;

            fn __upwell_ws_route_group() -> ::std::vec::Vec<#ws_route_descriptor> {
                ::std::vec![ #(#descriptors),* ]
            }

            #register
        };
    });
}
