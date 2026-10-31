use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{
    FnArg, GenericArgument, Ident, ImplItemFn, LitStr, PathArguments, ReturnType, Type, parse_quote,
};

use upwell_macros_core::attr::first_type_arg;
use upwell_macros_core::paths::Paths;

use crate::{client, route};

/// One WebSocket message route claimed from a `#[message("dest")]` method.
pub(super) struct WsRouteSpec {
    pub(super) destination: LitStr,
    pub(super) builder: TokenStream,
    pub(super) handler_name: Ident,
    /// The handler's entry-key segment: its name in `snake_case`.
    pub(super) key_segment: LitStr,
    pub(super) payload: Option<Type>,
    pub(super) is_request: bool,
    pub(super) reply: Option<Type>,
    pub(super) codec: TokenStream,
    /// The `T` of every `Inject<T>` parameter, in parameter order.
    pub(super) targets: Vec<Type>,
}

/// Builds the message-route builder fragment for one `#[message("dest")]` method.
pub(super) fn build_ws_route(
    self_ty: &Type,
    protocol: &syn::Path,
    method: &ImplItemFn,
    destination: &LitStr,
    codec: &TokenStream,
    is_request: bool,
    paths: &Paths,
) -> syn::Result<WsRouteSpec> {
    let takes_self = matches!(method.sig.inputs.first(), Some(FnArg::Receiver(_)));

    if let Some(FnArg::Receiver(receiver)) = method.sig.inputs.first()
        && (receiver.reference.is_none() || receiver.mutability.is_some())
    {
        return Err(syn::Error::new_spanned(
            receiver,
            "ws message methods may take `&self` only (the controller singleton is shared; \
             `self` by value and `&mut self` are not allowed)",
        ));
    }

    let ws_route = paths.plugin("WsRoute");
    let ws_protocol = paths.plugin("WebsocketProtocol");
    let ws_respond = paths.plugin("WsRespond");
    let inject = paths.plugin("Inject");
    let scope_container = paths.plugin("__ScopeContainer");
    let dispatch_error = paths.plugin("WsDispatchError");
    let proto = quote!(#protocol);
    let payload_ty = quote!(<#proto as #ws_protocol>::Payload);
    let ws_future = paths.plugin("ws::WsFuture");
    let ws_future_p = quote!(#ws_future<#proto>);
    let mut bindings: Vec<TokenStream> = Vec::new();
    let mut call_args: Vec<Ident> = Vec::new();
    let mut payload_seen = false;
    let mut targets: Vec<Type> = Vec::new();

    for (i, arg) in method.sig.inputs.iter().enumerate() {
        let FnArg::Typed(typed) = arg else {
            continue;
        };
        let ident = format_ident!("__a{i}");
        let ty = typed.ty.as_ref();

        match inject_inner(ty) {
            Some(handle) => {
                targets.push(handle.clone());
                bindings.push(quote! {
                    let #ident = #inject(
                        __scope.extract::<#handle>().await.map_err(|__e| {
                            #dispatch_error::Inject(::std::string::ToString::to_string(&__e))
                        })?,
                    );
                });
            }
            None => {
                if payload_seen {
                    return Err(syn::Error::new_spanned(
                        ty,
                        "a ws message method takes at most one payload parameter (the frame \
                         carries one JSON payload); other parameters must be `Inject<T>`",
                    ));
                }
                payload_seen = true;
                let topic_codec_trait = paths.plugin("TopicCodec");
                let decode = quote!(
                    <#codec as #topic_codec_trait<#proto>>::decode::<#ty>(__payload)
                        .map_err(|__e| #dispatch_error::Decode(::std::string::ToString::to_string(&__e)))?
                );
                bindings.push(quote!(let #ident: #ty = #decode;));
            }
        }
        call_args.push(ident);
    }

    if !payload_seen {
        bindings.insert(0, quote!(let _ = &__payload;));
    }

    let method_ident = &method.sig.ident;
    let dotawait = if method.sig.asyncness.is_some() {
        quote!(.await)
    } else {
        quote!()
    };
    let invoke = if takes_self {
        quote!(<#self_ty>::#method_ident(&__svc, #(#call_args),*)#dotawait)
    } else {
        quote!(<#self_ty>::#method_ident(#(#call_args),*)#dotawait)
    };
    let outcome_expr = if is_request {
        let topic_codec = paths.plugin("TopicCodec");
        let message_reply = paths.plugin("MessageReply");
        let ok_value = message_reply_ok_value(&method.sig.output, &dispatch_error);
        quote! {
            let __ok = #ok_value;
            let __body = <#codec as #topic_codec<#proto>>::encode(&__ok)
                .map_err(|__e| #dispatch_error::Encode(::std::string::ToString::to_string(&__e)))?;

            ::core::result::Result::Ok(<#proto as #message_reply>::reply(__body))
        }
    } else {
        let (response_ty, response_value) =
            message_success_value(&method.sig.output, false, &dispatch_error);
        quote!(<#proto as #ws_respond<#response_ty>>::respond(#response_value))
    };
    let builder = quote! {{
        let __svc = ::std::sync::Arc::clone(&svc);

        #ws_route::new(
            #destination,
            ::std::sync::Arc::new(
                move |__payload: #payload_ty, __scope: ::std::sync::Arc<#scope_container>|
                    -> #ws_future_p {
                    let __svc = ::std::sync::Arc::clone(&__svc);

                    ::std::boxed::Box::pin(async move {
                        #(#bindings)*
                        let __resp = #invoke;

                        #outcome_expr
                    })
                },
            ),
        )
    }};

    Ok(WsRouteSpec {
        destination: destination.clone(),
        builder,
        handler_name: method.sig.ident.clone(),
        key_segment: upwell_macros_core::entry::handler_segment(&method.sig.ident)?,
        payload: ws_payload_type(method)?,
        is_request,
        reply: is_request.then(|| client::response_type(&method.sig.output)),
        codec: codec.clone(),
        targets,
    })
}

pub(super) fn ws_payload_type(method: &ImplItemFn) -> syn::Result<Option<Type>> {
    let mut payload: Option<Type> = None;

    for arg in &method.sig.inputs {
        let FnArg::Typed(typed) = arg else {
            continue;
        };
        let ty = typed.ty.as_ref();
        if inject_inner(ty).is_some() {
            continue;
        }
        if payload.replace(ty.clone()).is_some() {
            return Err(syn::Error::new_spanned(
                ty,
                "a ws message method takes at most one payload parameter (the frame carries one \
                 payload); other parameters must be `Inject<T>`",
            ));
        }
    }
    Ok(payload)
}

pub(super) fn resolve_message_reply(mode: route::MessageMode, output: &ReturnType) -> bool {
    let returns_value =
        !matches!(client::response_type(output), Type::Tuple(ref tuple) if tuple.elems.is_empty());
    match mode {
        route::MessageMode::Send => false,
        route::MessageMode::Request => true,
        route::MessageMode::Infer => returns_value,
    }
}

fn message_reply_ok_value(output: &ReturnType, dispatch_error: &syn::Path) -> TokenStream {
    message_success_value(output, true, dispatch_error).1
}

pub(super) fn message_success_value(
    output: &ReturnType,
    peel_json: bool,
    dispatch_error: &syn::Path,
) -> (Type, TokenStream) {
    let ty = match output {
        ReturnType::Type(_, ty) => (**ty).clone(),
        ReturnType::Default => return (parse_quote!(()), quote!(__resp)),
    };
    let mut expr = quote!(__resp);
    let inner = if let Some(ok) = first_type_arg(&ty, "Result") {
        expr = quote!((#expr).map_err(|_| #dispatch_error::Application)?);
        ok
    } else {
        ty
    };
    let json = first_type_arg(&inner, "Json");
    let success = if peel_json {
        json.clone().unwrap_or(inner)
    } else {
        inner
    };
    if peel_json && json.is_some() {
        expr = quote!((#expr).0);
    }
    (success, expr)
}

fn inject_inner(ty: &Type) -> Option<&Type> {
    let Type::Path(type_path) = ty else {
        return None;
    };
    let segment = type_path.path.segments.last()?;
    if segment.ident != "Inject" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    args.args.iter().find_map(|arg| match arg {
        GenericArgument::Type(inner) => Some(inner),
        _ => None,
    })
}
