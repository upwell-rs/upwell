//! The RPC handlers extension: `Rpcs`, the `ParseMethod` extension that makes
//! `#[handlers]` = `MethodArgs<Rpcs>` (`#[methods]` + RPC registration).
//!
//! `Rpcs` claims each `#[rpc]` method — building its erased dispatch wrapper and its
//! `RpcDescriptor`, and returning a [`ClientMethod`] hint so the **framework** generates the
//! client (the client is protocol-agnostic). On emission `Rpcs` contributes only the RPC-
//! specific surface: the wrappers and the `RpcGroup` registration appended to the service's
//! `{Service}Rpcs` slice. The base [`MethodArgs`](upwell_macros_core::methods::MethodArgs)
//! handles `#[init]`/`#[hook]`, so a `#[handlers]` block supports those too.

use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::parse::ParseStream;
use syn::{
    Attribute, FnArg, Ident, ImplItemFn, ItemImpl, LitStr, Meta, ReturnType, Type, spanned::Spanned,
};

use upwell_macros_core::attr;
use upwell_macros_core::client::{Capability, ClientMethod};
use upwell_macros_core::extend::{ParseItem, ParseKeyed, ParseMethod, eat_eq};
use upwell_macros_core::methods::self_ty_ident;
use upwell_macros_core::paths::Paths;

/// The RPC handlers extension. Accumulates the impl's `#[rpc]` wrappers and descriptors and the
/// captured impl context, then emits the wrappers and the RPC group registration. The client
/// methods are *not* held here — they are returned from [`ParseMethod::parse_method`] as hints
/// the framework turns into the generated client.
#[derive(Default)]
pub struct Rpcs {
    /// `rpc_slice = ..` — the per-service slice to append to (default `{Service}Rpcs`).
    rpc_slice: Option<Ident>,
    /// `client_trait = ..` — accepted for back-compat, no longer changes codegen.
    client_trait: Option<Ident>,
    /// Captured during [`ParseItem`]: the impl's self type, ident, and name.
    context: Option<RpcContext>,
    /// Accumulated per `#[rpc]` method (during [`ParseMethod`]).
    wrappers: Vec<TokenStream>,
    descriptors: Vec<TokenStream>,
}

/// The impl context `Rpcs` needs to emit (captured in the item pass).
struct RpcContext {
    self_ty: Type,
    self_ident: Ident,
    self_name: LitStr,
    paths: Paths,
}

impl ParseKeyed for Rpcs {
    fn parse_keyed(&mut self, key: &Ident, input: ParseStream) -> syn::Result<bool> {
        match key.to_string().as_str() {
            "rpc_slice" => {
                eat_eq(input)?;
                self.rpc_slice = Some(input.parse()?);

                Ok(true)
            }

            "client_trait" => {
                eat_eq(input)?;
                self.client_trait = Some(input.parse()?);

                Ok(true)
            }

            _ => Ok(false),
        }
    }

    fn expected_keys() -> &'static [&'static str] {
        &["rpc_slice", "client_trait"]
    }
}

impl ParseItem<ItemImpl> for Rpcs {
    fn parse_item(&mut self, item: &ItemImpl, paths: &Paths) -> syn::Result<()> {
        let self_ty = (*item.self_ty).clone();
        let self_ident = self_ty_ident(&self_ty)?;
        let self_name = LitStr::new(&self_ident.to_string(), self_ident.span());

        self.context = Some(RpcContext {
            self_ty,
            self_ident,
            self_name,
            paths: paths.clone(),
        });

        Ok(())
    }
}

impl ParseMethod for Rpcs {
    fn parse_method(&mut self, method: &mut ImplItemFn) -> syn::Result<Option<ClientMethod>> {
        let Some(pos) = method.attrs.iter().position(|a| a.path().is_ident("rpc")) else {
            return Ok(None);
        };

        let rpc_attr = method.attrs.remove(pos);
        let stream_flag = parse_rpc_attr(&rpc_attr)?;

        // `parse_item` runs before the method walk, so the context is always present.
        let cx = self
            .context
            .as_ref()
            .expect("Rpcs::parse_item runs before parse_method");

        let (wrapper, descriptor, client) = expand_method(
            &cx.self_ty,
            &cx.self_ident,
            &cx.self_name,
            method,
            stream_flag,
            &cx.paths,
        )?;

        self.wrappers.push(wrapper);
        self.descriptors.push(descriptor);

        // Hand the client-method hint back to the framework, which owns client generation.
        Ok(Some(client))
    }
}

impl ToTokens for Rpcs {
    fn to_tokens(&self, out: &mut TokenStream) {
        // `client_trait =` is superseded by capability-gated inherent methods; kept only for
        // back-compat parsing.
        let _ = &self.client_trait;

        let Some(cx) = &self.context else {
            return;
        };

        if self.descriptors.is_empty() {
            return;
        }

        let paths = &cx.paths;
        let count = self.descriptors.len();
        let descriptors = &self.descriptors;
        let wrappers = &self.wrappers;
        let distributed_slice = paths.core("linkme::distributed_slice");
        let linkme_crate = paths.core("linkme");
        let inventory = paths.core("inventory");
        let descriptor_for = paths.core("DescriptorFor");
        let rpc_descriptor = paths.plugin("RpcDescriptor");
        let rpc_group = paths.plugin("RpcGroup");
        let rpcs_slice = self
            .rpc_slice
            .clone()
            .unwrap_or_else(|| format_ident!("{}Rpcs", cx.self_ident));
        let type_descriptor = paths.core("TypeDescriptor");
        let self_ty = &cx.self_ty;
        let self_name = &cx.self_name;

        let group_literal = quote! {
            #rpc_group {
                service: #type_descriptor::of::<#self_ty>(#self_name),
                rpcs: &__UPWELL_RPCS,
            }
        };
        // One `RpcGroup` per `#[handlers]` block, registered into the service's per-type registry.
        let register = upwell_macros_core::backend::dual_backend(
            quote! {
                #inventory::submit! {
                    #descriptor_for::<#self_ty, #rpc_group>::new(#group_literal)
                }
            },
            quote! {
                #[#distributed_slice(#rpcs_slice)]
                #[linkme(crate = #linkme_crate)]
                static __UPWELL_RPC_GROUP: #rpc_group = #group_literal;
            },
        );

        // The RPC-specific surface only: the dispatch wrappers and the group registration. The
        // client is emitted by the framework from the per-method hints `parse_method` returned.
        out.extend(quote! {
            const _: () = {
                #(#wrappers)*

                static __UPWELL_RPCS: [#rpc_descriptor; #count] = [
                    #(#descriptors),*
                ];

                #register
            };
        });
    }
}

/// Parses the `#[rpc]` attribute, returning whether the `stream` flag is set.
///
/// `#[rpc]` infers the RPC kind from the signature; the sole argument
/// `#[rpc(stream)]` marks a *concrete* return type as a server stream for the
/// cases the macro cannot otherwise see it is a `Stream` (it only flips the
/// operation kind — item serialization is unchanged).
fn parse_rpc_attr(attr: &Attribute) -> syn::Result<bool> {
    match &attr.meta {
        Meta::Path(_) => Ok(false),

        Meta::List(list) => {
            let ident: Ident = list.parse_args()?;

            if ident == "stream" {
                Ok(true)
            } else {
                Err(syn::Error::new_spanned(
                    &list.tokens,
                    "unknown #[rpc] argument; the only argument is `stream` (mark a concrete \
                     return type as a server stream)",
                ))
            }
        }

        Meta::NameValue(meta) => Err(syn::Error::new_spanned(
            meta,
            "#[rpc] takes no name-value arguments",
        )),
    }
}

/// The streaming shape resolved from a handler signature, threaded into the
/// client codegen: which directions stream, the value item types the client
/// sends/receives (1:1 with the daemon), and a streaming output's per-item error.
struct StreamTypes {
    streamed_input: bool,
    streamed_output: bool,
    req_item: Option<Type>,
    resp_item: Option<Type>,
    stream_err: Option<Type>,
}

/// How the dispatch wrapper adapts a streaming return value into the canonical
/// `ResponseStream` before it reaches the `Responder` layer.
enum OutputWrap {
    /// Already a `Responder` (an explicit `ResponseStream<T>`); pass through.
    None,
    /// `impl Stream<Item = T>` — wrap with `ResponseStream::from_items`.
    Items,
    /// `impl Stream<Item = Result<T, E>>` — wrap with `ResponseStream::from_results`.
    Results,
}

/// Builds the erased handler wrapper, `RpcDescriptor`, and client method for one
/// `#[rpc]` method.
fn expand_method(
    self_ty: &Type,
    self_ident: &syn::Ident,
    self_name: &LitStr,
    method: &ImplItemFn,
    stream_flag: bool,
    paths: &Paths,
) -> syn::Result<(TokenStream, TokenStream, ClientMethod)> {
    if method.sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            &method.sig,
            "rpc methods must be `async`",
        ));
    }

    let takes_self = match method.sig.inputs.first() {
        Some(FnArg::Receiver(receiver)) => {
            if receiver.reference.is_none() || receiver.mutability.is_some() {
                return Err(syn::Error::new_spanned(
                    receiver,
                    "rpc methods may take `&self` only (the service singleton is shared; \
                     `self` by value and `&mut self` are not allowed)",
                ));
            }

            true
        }
        _ => false,
    };

    let param_types: Vec<&Type> = method
        .sig
        .inputs
        .iter()
        .filter_map(|arg| match arg {
            FnArg::Typed(typed) => Some(typed.ty.as_ref()),
            FnArg::Receiver(_) => None,
        })
        .collect();

    let generics = &method.sig.generics;
    let streaming = paths.plugin("Streaming");
    let request_stream = paths.plugin("RequestStream");

    // Resolve each parameter into the concrete extractor type the dispatch
    // closure declares (so it always flows through `FromContext`), and detect the
    // at-most-one streaming input. A `Streaming<T>` parameter keeps its fallible
    // items; an `impl Stream<Item = T>` / generic `S: Stream<..>` parameter maps
    // to `RequestStream<T>` (or `Streaming<T>` when its items are `Result`s) so
    // the handler receives exactly the item shape it declared.
    let mut extractor_types: Vec<TokenStream> = Vec::new();
    let mut streaming_inputs = 0usize;
    let mut has_payload = false;
    let mut req_item: Option<Type> = None;

    for ty in &param_types {
        if attr::is_payload_param(ty) {
            has_payload = true;
        }

        if attr::is_streaming_param(ty) {
            streaming_inputs += 1;
            req_item = attr::streaming_inner(ty);
            extractor_types.push(quote!(#ty));

            continue;
        }

        if let Some(shape) = attr::stream_shape(ty, generics) {
            let item = &shape.item;
            streaming_inputs += 1;
            req_item = Some(item.clone());

            if shape.fallible() {
                extractor_types.push(quote!(#streaming<#item>));
            } else {
                extractor_types.push(quote!(#request_stream<#item>));
            }

            continue;
        }

        extractor_types.push(quote!(#ty));
    }

    if streaming_inputs > 1 {
        return Err(syn::Error::new_spanned(
            &method.sig.inputs,
            "an rpc method may take at most one streaming-input parameter",
        ));
    }

    let streamed_input = streaming_inputs == 1;

    if streamed_input && has_payload {
        return Err(syn::Error::new_spanned(
            &method.sig.inputs,
            "a streaming-input rpc reads its request from its stream parameter; \
             remove the `Payload<T>` parameter",
        ));
    }

    // Output streaming: an explicit `ResponseStream<T>` return (already a
    // `Responder`, so no wrapping), an `impl Stream` / generic stream return
    // (wrapped per its item shape), or a concrete return flagged `#[rpc(stream)]`
    // (wrapped as plain items — the macro cannot introspect its shape).
    let return_ty = match &method.sig.output {
        ReturnType::Type(_, ty) => Some(ty.as_ref()),
        ReturnType::Default => None,
    };
    let return_shape = return_ty.and_then(|ty| attr::stream_shape(ty, generics));
    let returns_response_stream = attr::returns_response_stream(&method.sig.output);

    let (streamed_output, output_wrap, resp_item, stream_err): (
        bool,
        OutputWrap,
        Option<Type>,
        Option<Type>,
    ) = if returns_response_stream {
        let item = attr::response_body_type(&method.sig.output);
        let err = attr::result_type_args(&method.sig.output).and_then(|(_, err)| err);

        (true, OutputWrap::None, Some(item), err)
    } else if let Some(shape) = return_shape {
        let wrap = if shape.fallible() {
            OutputWrap::Results
        } else {
            OutputWrap::Items
        };

        (true, wrap, Some(shape.item), shape.error)
    } else if stream_flag {
        // A concrete return the macro cannot introspect: serialize each item as-is,
        // and recover the wire item type by projecting through the `Stream` trait so
        // the client stays well-typed (`<ReturnType as Stream>::Item`).
        let stream_trait = paths.plugin("__Stream");
        let item = return_ty.map(|ty| syn::parse_quote!(<#ty as #stream_trait>::Item));

        (true, OutputWrap::Items, item, None)
    } else {
        (false, OutputWrap::None, None, None)
    };

    let operation = attr::operation_ident(streamed_input, streamed_output);

    let method_ident = &method.sig.ident;
    let method_name = LitStr::new(&method_ident.to_string(), method_ident.span());

    // Descriptor metadata uses the streamed item type (not the stream wrapper,
    // which may be an un-nameable `impl Trait`); a non-streaming output keeps the
    // peeled response body type.
    let output_ty: Type = if streamed_output {
        resp_item.clone().unwrap_or_else(|| syn::parse_quote!(()))
    } else {
        attr::response_body_type(&method.sig.output)
    };
    let output_name = LitStr::new(&output_ty.to_token_stream().to_string(), output_ty.span());

    let wrapper_ident = format_ident!(
        "__upwell_rpc_{}_{}",
        self_ident.to_string().to_lowercase(),
        method_ident
    );

    // A `Result` return dispatches through `FallibleHandler` (which enforces
    // `E: ResponseError`); any other `Responder` return goes through `Handler`.
    // Both erase to the same `RpcHandler` fn pointer.
    let dispatch = if attr::returns_result(&method.sig.output) {
        paths.plugin("dispatch_fallible")
    } else {
        paths.plugin("dispatch_with")
    };
    let error = paths.plugin("Error");
    let operation_kind = paths.plugin("OperationKind");
    let response_stream = paths.plugin("ResponseStream");
    let rpc_call_context = paths.plugin("RpcCallContext");
    let rpc_descriptor = paths.plugin("RpcDescriptor");
    let type_descriptor = paths.core("TypeDescriptor");
    let entry_key_owner = paths.core("EntryKeyOwner");
    let from_container = paths.core("FromContainer");
    let entry_key = paths.core("entry_key");
    let ret = handler_return_type(paths);
    let key_segment = upwell_macros_core::entry::handler_segment(method_ident)?;
    let targets = param_types
        .iter()
        .filter_map(|ty| attr::first_type_arg(ty, "Inject"))
        .map(|ty| quote!(<#ty as #from_container>::dependency()));

    let wrap = |inner: TokenStream| match output_wrap {
        OutputWrap::None => inner,
        OutputWrap::Items => quote!(#response_stream::from_items(#inner)),
        OutputWrap::Results => quote!(#response_stream::from_results(#inner)),
    };

    let arg_idents: Vec<_> = (0..param_types.len())
        .map(|i| format_ident!("__a{i}"))
        .collect();

    let wrapper = if takes_self {
        let body = wrap(quote!(<#self_ty>::#method_ident(&__svc, #(#arg_idents),*).await));

        quote! {
            #[allow(non_snake_case)]
            fn #wrapper_ident(ctx: #rpc_call_context) -> #ret {
                ::std::boxed::Box::pin(async move {
                    let __svc = ctx
                        .component::<#self_ty>()
                        .ok_or(#error::MissingComponent(#self_name))?;

                    #dispatch(
                        move |#(#arg_idents: #extractor_types),*| {
                            let __svc = ::std::sync::Arc::clone(&__svc);

                            async move { #body }
                        },
                        ctx,
                    )
                    .await
                })
            }
        }
    } else {
        let body = wrap(quote!(<#self_ty>::#method_ident(#(#arg_idents),*).await));

        quote! {
            #[allow(non_snake_case)]
            fn #wrapper_ident(ctx: #rpc_call_context) -> #ret {
                #dispatch(
                    move |#(#arg_idents: #extractor_types),*| async move { #body },
                    ctx,
                )
            }
        }
    };

    let source = upwell_macros_core::source::item_source(
        method_ident.span(),
        &upwell_macros_core::source::associated_item(self_ty, method_ident),
        paths,
    );
    let descriptor = quote! {
        #rpc_descriptor {
            name: #method_name,
            operation: #operation_kind::#operation,
            parameters: &[],
            output: #type_descriptor::of::<#output_ty>(#output_name),
            handler: #wrapper_ident,
            source: ::core::option::Option::Some(#source),
            key: #entry_key!(
                "rpc/",
                <#self_ty as #entry_key_owner>::KEY_SEGMENT,
                ".",
                #key_segment,
            ),
            targets: || ::std::vec![#(#targets),*],
        }
    };

    let stream_types = StreamTypes {
        streamed_input,
        streamed_output,
        req_item,
        resp_item,
        stream_err,
    };
    let client = client_method(
        self_ident,
        method_ident,
        &param_types,
        &method.sig.output,
        &stream_types,
        paths,
    );

    Ok((wrapper, descriptor, client))
}

/// Builds the framework's [`ClientMethod`] *hint* for one `#[rpc]`, resolving the RPC-specific
/// types: the request body from `Payload<T>`, the stream item types from the streaming shapes,
/// the success type from the return, and the decoded error-body type from
/// `<E as ResponseError>::Body`. The framework owns turning this hint into the generated
/// method (signature + body).
fn client_method(
    self_ident: &syn::Ident,
    method_ident: &syn::Ident,
    param_types: &[&Type],
    output: &ReturnType,
    stream: &StreamTypes,
    paths: &Paths,
) -> ClientMethod {
    let StreamTypes {
        streamed_input,
        streamed_output,
        req_item,
        resp_item,
        stream_err,
    } = stream;
    let (streamed_input, streamed_output) = (*streamed_input, *streamed_output);
    let response_error = paths.plugin("ResponseError");

    let payload_ty = param_types.iter().find_map(|ty| attr::payload_inner(ty));
    let (result_ok, result_err) = match attr::result_type_args(output) {
        Some((ok, err)) => (Some(ok), err),
        None => (None, None),
    };

    // The client decodes the *body* the error serializes, which may differ from the handler's
    // error type — so it tracks `<E as ResponseError>::Body`, not `E`. A streaming output's
    // per-item error wins; otherwise the unary result error. A hidden error (one-argument
    // `Result<T>`, or an infallible stream) leaves the framework's default `Raw`.
    let err_source = stream_err.clone().or(result_err);
    let error_ty = err_source
        .as_ref()
        .map(|e| quote!(<#e as #response_error>::Body));

    // The unary/client-streaming success type: the `Ok` of a `Result`, else the bare return
    // type, else `()`. `Option<T>` is left intact.
    let response: Type = match (&result_ok, output) {
        (Some(ok), _) => ok.clone(),
        (None, ReturnType::Type(_, ty)) => (**ty).clone(),
        (None, ReturnType::Default) => syn::parse_quote!(()),
    };

    let capability = match (streamed_input, streamed_output) {
        (false, false) => Capability::Unary,
        (false, true) => Capability::ServerStreaming,
        (true, false) => Capability::ClientStreaming,
        (true, true) => Capability::BidiStreaming,
    };

    ClientMethod {
        ident: method_ident.clone(),
        path: format!("{self_ident}.{method_ident}"),
        capability,
        request: payload_ty,
        encode_as: None,
        req_item: req_item.clone(),
        resp_item: resp_item.clone(),
        response,
        error_ty,
        // RPC is a pure passthrough: no extra args, both envelopes are the body/decoded value,
        // and no request build, response map, or override step.
        extra_args: Vec::new(),
        request_envelope: None,
        request_builder: None,
        response_envelope: None,
        response_mapper: None,
        trailing_args: ::std::vec::Vec::new(),
        attrs: ::std::vec::Vec::new(),
        override_bounds: None,
        override_ret: None,
        override_body: None,
    }
}

/// The erased `RpcHandler` return type, repeated by both wrapper forms.
fn handler_return_type(paths: &Paths) -> TokenStream {
    let error_response = paths.plugin("ErrorResponse");
    let rpc_outcome = paths.plugin("RpcOutcome");

    quote! {
        ::core::pin::Pin<
            ::std::boxed::Box<
                dyn ::core::future::Future<
                    Output = ::core::result::Result<#rpc_outcome, #error_response>,
                > + ::core::marker::Send,
            >,
        >
    }
}
