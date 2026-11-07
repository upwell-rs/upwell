use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, Ident, ImplItemFn, LitStr, Path, Type};

use upwell_macros_core::paths::Paths;

use crate::client;
use crate::route::RouteAttr;

pub(super) struct HttpRouteContext<'a> {
    pub(super) stream_param: Option<&'a (usize, Type)>,
    pub(super) server_wrap: Option<&'a client::ServerWrap>,
    pub(super) stream_item: Option<&'a Type>,
    pub(super) is_streaming: bool,
    pub(super) in_result: bool,
}

/// One route claimed from a method.
pub(super) struct RouteSpec {
    pub(super) handler_name: Ident,
    /// The handler's entry-key segment, validated at the method.
    pub(super) key_segment: LitStr,
    pub(super) verb: Ident,
    pub(super) path: LitStr,
    pub(super) middleware: Vec<Path>,
    pub(super) handler: TokenStream,
    pub(super) inputs: Vec<crate::http_analysis::Input>,
    pub(super) path_parameters: Vec<(String, bool)>,
    pub(super) output: crate::http_analysis::Output,
}

/// Builds the typed axum handler closure for one route-attributed method.
pub(super) fn build_route(
    self_ty: &Type,
    method: &ImplItemFn,
    route_attr: &RouteAttr,
    route_context: HttpRouteContext<'_>,
    paths: &Paths,
) -> syn::Result<RouteSpec> {
    let takes_self = match method.sig.inputs.first() {
        Some(FnArg::Receiver(receiver)) => {
            if receiver.reference.is_none() || receiver.mutability.is_some() {
                return Err(syn::Error::new_spanned(
                    receiver,
                    "controller route methods may take `&self` only (the controller singleton \
                     is shared; `self` by value and `&mut self` are not allowed)",
                ));
            }
            true
        }
        _ => false,
    };

    let arg_types: Vec<&Type> = method
        .sig
        .inputs
        .iter()
        .filter_map(|arg| match arg {
            FnArg::Typed(typed) => Some(typed.ty.as_ref()),
            FnArg::Receiver(_) => None,
        })
        .collect();
    let arg_idents: Vec<Ident> = (0..arg_types.len())
        .map(|i| format_ident!("__a{i}"))
        .collect();

    let stream_body = paths.plugin("StreamBody");
    let closure_params: Vec<TokenStream> = arg_types
        .iter()
        .zip(&arg_idents)
        .enumerate()
        .map(|(i, (ty, ident))| match route_context.stream_param {
            Some((index, item)) if *index == i => quote!(#ident: #stream_body<#item>),
            _ => quote!(#ident: #ty),
        })
        .collect();
    let call_args: Vec<TokenStream> = arg_idents
        .iter()
        .enumerate()
        .map(|(i, ident)| match route_context.stream_param {
            Some((index, _)) if *index == i => quote!(#ident.into_stream()),
            _ => quote!(#ident),
        })
        .collect();
    let method_ident = &method.sig.ident;
    let dotawait = if method.sig.asyncness.is_some() {
        quote!(.await)
    } else {
        quote!()
    };

    let wrap = |call: TokenStream| {
        let wrapper = match route_context.server_wrap {
            Some(client::ServerWrap::Ndjson) => {
                let ndjson = paths.plugin("Ndjson");
                quote!(#ndjson)
            }
            Some(client::ServerWrap::RawU8) => {
                let raw = paths.plugin("RawStream");
                let chunk_u8 = paths.plugin("chunk_u8");
                quote!(|__stream| #raw(#chunk_u8(__stream)))
            }
            None => return call,
        };

        if route_context.in_result {
            quote!(#call.map(#wrapper))
        } else {
            quote!((#wrapper)(#call))
        }
    };

    let handler = if takes_self {
        let call = wrap(quote!(<#self_ty>::#method_ident(&__svc, #(#call_args),*)#dotawait));
        quote! {{
            let __svc = ::std::sync::Arc::clone(&svc);

            move |#(#closure_params),*| {
                let __svc = ::std::sync::Arc::clone(&__svc);

                async move { #call }
            }
        }}
    } else {
        let call = wrap(quote!(<#self_ty>::#method_ident(#(#call_args),*)#dotawait));
        quote! { move |#(#closure_params),*| async move { #call } }
    };

    let inputs = method
        .sig
        .inputs
        .iter()
        .enumerate()
        .filter_map(|(index, argument)| {
            let FnArg::Typed(typed) = argument else {
                return None;
            };
            let name = match typed.pat.as_ref() {
                syn::Pat::Ident(ident) => ident.ident.to_string(),
                _ => format!("arg{index}"),
            };
            let ty = typed.ty.as_ref();
            let (source, semantic_ty) = if let Some((stream_index, item)) =
                route_context.stream_param
                && *stream_index == index.saturating_sub(usize::from(takes_self))
            {
                ("Stream", item.clone())
            } else {
                crate::http_analysis::input(ty)
            };
            Some(crate::http_analysis::Input {
                name,
                source,
                ty: semantic_ty,
            })
        })
        .collect();
    let (_, holes) = client::parse_template(&route_attr.path.value());
    let path_parameters = holes
        .into_iter()
        .map(|name| {
            let catch_all = name.starts_with('*');
            (name.trim_start_matches('*').to_string(), catch_all)
        })
        .collect();
    let output = crate::http_analysis::output(
        method,
        route_attr,
        route_context.server_wrap,
        route_context.stream_item,
        route_context.is_streaming,
        route_attr.streamed,
    );

    Ok(RouteSpec {
        handler_name: method.sig.ident.clone(),
        key_segment: upwell_macros_core::entry::handler_segment(&method.sig.ident)?,
        verb: route_attr.verb.clone(),
        path: route_attr.path.clone(),
        middleware: route_attr.middleware.clone(),
        handler,
        inputs,
        path_parameters,
        output,
    })
}
