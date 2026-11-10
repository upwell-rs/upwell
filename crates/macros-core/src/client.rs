//! The protocol-agnostic generated **client** — a framework responsibility.
//!
//! The generated `{Service}Client<C>` is transport-generic and capability-partitioned: it
//! roots entirely at `::upwell::client::*` (the agnostic capability contract), so the same
//! client works over any protocol that supplies those capabilities. The framework therefore
//! **owns the client generation**; a protocol macro only describes each method as a
//! [`ClientMethod`] *hint* — returned as a byproduct of its
//! [`ParseMethod::parse_method`](crate::ParseMethod::parse_method) — and the framework
//! assembles the method bodies and emits the capability-partitioned client from those hints.
//!
//! Why a hint and not the finished code: most of the type extraction (the request body, the
//! streamed item types, the error model) flows through *protocol-specific* extractors — RPC's
//! `Payload<T>` / `Streaming<T>` / `ResponseError` — so the protocol resolves the types; the
//! framework owns the (non-trivial, agnostic) assembly of the call signature and body.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Ident, Type};

use crate::paths::Paths;

/// Which client capability a method needs — selects the `impl<C: Cap>` block it lands in, so
/// the method exists only when the protocol `C` supports that call shape (an unsupported one is
/// simply absent, never a compile error).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Unary,
    ServerStreaming,
    ClientStreaming,
    BidiStreaming,
}

/// A protocol's description ("hint") of one client method. The protocol fills it from its own
/// signature analysis (which extractors mean a request body, a stream, an error type); the
/// framework assembles the call signature and body and emits the capability-partitioned client.
#[derive(Clone)]
pub struct ClientMethod {
    /// The generated method name.
    pub ident: Ident,
    /// The wire path the client calls (e.g. `"Service.method"`).
    pub path: String,
    /// The call shape.
    pub capability: Capability,
    /// The unary/server-streaming request value type the method *takes*, or `None` for a
    /// no-body call. For HTTP this is the raw `T` (e.g. `SumIn`), the ergonomic surface; the
    /// wire body type it is encoded as may differ — see [`encode_as`](Self::encode_as).
    pub request: Option<Type>,
    /// Overrides the wire body type (`Encodes<B>` and the request envelope's `B`) when it
    /// differs from the [`request`](Self::request) param type. `None` encodes the param type
    /// as-is (RPC). HTTP sets it to the `HttpBody` wrapper (`Json<T>`/`Form<T>`) while the
    /// param stays the raw `T`, and the `request_builder` wraps it.
    pub encode_as: Option<TokenStream>,
    /// The client/bidi-streaming request *item* type (`None` for non-streaming-input).
    pub req_item: Option<Type>,
    /// The server/bidi-streaming response *item* type (`None` for non-streaming-output).
    pub resp_item: Option<Type>,
    /// The unary/client-streaming success type.
    pub response: Type,
    /// The decoded error body type (protocol-specific — RPC uses `<E as ResponseError>::Body`).
    /// `None` leaves the framework default `::upwell::client::Raw` (an opaque error body).
    pub error_ty: Option<TokenStream>,
    /// Extra leading method parameters before the request body (HTTP path/query params).
    /// Empty for RPC. Spliced into the signature and visible to [`request_builder`](Self::request_builder).
    pub extra_args: Vec<(Ident, TokenStream)>,
    /// The request envelope type pinned by the `Unary<Request<B> = ..>` bound. `None` means the
    /// body passes through unchanged (`Request<B> = B`, RPC); `Some(ty)` pins a richer envelope
    /// (HTTP's `http::Request<B>`), so the generated method may construct it.
    pub request_envelope: Option<TokenStream>,
    /// Builds the request envelope value passed to the capability call. `None` forwards the body
    /// argument as-is (passthrough); `Some(expr)` builds it (HTTP constructs the `http::Request`).
    pub request_builder: Option<TokenStream>,
    /// The response envelope type pinned by the `Unary<Response<R> = ..>` bound, and the method's
    /// success return type. `None` returns the decoded body unchanged (`Response<R> = R`, RPC);
    /// `Some(ty)` returns a richer envelope (HTTP's `HttpResponse<R>`, which derefs to `R`).
    pub response_envelope: Option<TokenStream>,
    /// Maps the call result before returning. `None` returns it unchanged; `Some(expr)` is applied
    /// as `.map(#expr)` — for a protocol whose response needs a post-step the codec can't express.
    pub response_mapper: Option<TokenStream>,

    /// **Override** — replaces the whole where-clause for this method (`C: ..`) when the standard
    /// capability bound does not fit. HTTP server-streaming sets `C: HttpStreaming + Encodes<B>`,
    /// since it calls an HTTP-specific capability rather than the agnostic one. `None` uses the
    /// capability default (`C: <Cap> + Encodes<..> + Decodes<..>`).
    pub override_bounds: Option<TokenStream>,
    /// **Override** — replaces the method's return type. `None` uses the capability default.
    pub override_ret: Option<TokenStream>,
    /// **Override** — replaces the method body. `None` uses the capability default
    /// (`self.0.<call>(..)`). The framework still assembles the signature; only the body is the
    /// protocol's, for a call the standard capability cannot express (e.g. byte stream + decode).
    pub override_body: Option<TokenStream>,

    /// Extra parameters appended **after** the body (mirroring [`extra_args`](Self::extra_args),
    /// which lead). Each `(name, type)` becomes a `, name: type` in the signature and is in scope for
    /// the method body / `request_builder`. Empty for most methods; a protocol uses it for an extra
    /// trailing input a variant method carries (e.g. the axum HTTP `_with_headers` variant).
    pub trailing_args: Vec<(Ident, TokenStream)>,

    /// Attributes emitted on the generated `pub async fn` (e.g. `#[inline(always)]` on a thin
    /// forwarding method). Empty for most methods.
    pub attrs: Vec<TokenStream>,
}

impl ClientMethod {
    /// Assembles this method's `(args, return type, body, where-bounds)` from its hint. The
    /// override hints ([`override_ret`](Self::override_ret) / `override_body` / `override_bounds`)
    /// replace the capability-derived pieces when a method's call cannot be expressed by the
    /// standard capability (e.g. HTTP server-streaming, which calls an HTTP-specific capability
    /// and decodes the body stream).
    fn build(&self, paths: &Paths) -> (TokenStream, TokenStream, TokenStream, TokenStream) {
        let client_error = paths.client("ClientError");
        let transport = paths.client("Transport");
        let stream_arg = paths.client("StreamArg");
        let server_streaming = paths.client("ServerStreaming");
        let client_streaming = paths.client("ClientStreaming");
        let bidi_streaming = paths.client("BidiStreaming");
        let unary = paths.client("Unary");
        let encodes = paths.client("Encodes");
        let decodes = paths.client("Decodes");

        let raw = paths.client("Raw");
        let path = &self.path;
        let response = &self.response;
        let err = self.error_ty.clone().unwrap_or_else(|| quote!(#raw));

        // The transport's protocol-defined status, threaded into every `ClientError<S, E>`. It is
        // a projection off the bound `C` (every capability requires `C: Transport`), so the client
        // surfaces RPC's packed status or HTTP's `http::StatusCode` without naming either.
        let status = quote!(<C as #transport>::Status);

        // The unary/server-stream request param and the value forwarded (a no-body call sends
        // the unit body, so the protocol must `Encodes<()>`). The *wire body* type
        // (`unary_encode`) may differ from the param type when `encode_as` is set — HTTP takes a
        // raw `T` but encodes `Json<T>`.
        let (req_arg, call_arg, request_ty) = match &self.request {
            Some(req) => (quote!(, request: #req), quote!(request), quote!(#req)),
            None => (quote!(), quote!(()), quote!(())),
        };
        let unary_encode = self.encode_as.clone().unwrap_or(request_ty);

        // Trailing parameters, appended after the body (empty for most methods). Like `extra_params`,
        // in scope for `request_builder`.
        let trailing_params = self
            .trailing_args
            .iter()
            .map(|(name, ty)| quote!(, #name: #ty));
        let trailing_params = quote!(#(#trailing_params)*);

        // Leading path/query parameters (HTTP); empty for RPC. Spliced before the body param
        // and visible to `request_builder`.
        let extra_params = self
            .extra_args
            .iter()
            .map(|(name, ty)| quote!(, #name: #ty));
        let extra_params = quote!(#(#extra_params)*);
        let req_item = self
            .req_item
            .as_ref()
            .map(|t| quote!(#t))
            .unwrap_or_else(|| quote!(()));
        let resp_item = self
            .resp_item
            .clone()
            .unwrap_or_else(|| self.response.clone());

        // The capability default `(args, ret, body, bounds)`. Bounds carry the full `C: ..`
        // predicate (capability + message codecs), so one `impl<C>` block can host every method.
        let (args, ret, body, bounds) = match self.capability {
            Capability::Unary => {
                let call = self
                    .request_builder
                    .clone()
                    .unwrap_or_else(|| call_arg.clone());
                let req_env = self
                    .request_envelope
                    .clone()
                    .unwrap_or_else(|| unary_encode.clone());
                let resp_env = self
                    .response_envelope
                    .clone()
                    .unwrap_or_else(|| quote!(#response));
                let map = match &self.response_mapper {
                    Some(mapper) => quote!(.map(#mapper)),
                    None => quote!(),
                };

                (
                    quote!(&self #extra_params #req_arg #trailing_params),
                    quote!(::core::result::Result<#resp_env, #client_error<#status, #err>>),
                    quote!(self.0.unary(#path, #call).await #map),
                    quote! {
                        C: #unary<Request<#unary_encode> = #req_env, Response<#response> = #resp_env>
                            + #encodes<#unary_encode>
                            + #decodes<#response>
                    },
                )
            }

            Capability::ServerStreaming => (
                quote!(&self #extra_params #req_arg),
                quote! {
                    ::core::result::Result<
                        <C as #server_streaming>::Responses<#resp_item, #err>,
                        #client_error<#status, #err>,
                    >
                },
                quote!(self.0.server_stream(#path, #call_arg).await),
                quote!(C: #server_streaming + #encodes<#unary_encode> + #decodes<#resp_item>),
            ),

            Capability::ClientStreaming => (
                quote! {
                    &self #extra_params,
                    input: impl ::core::convert::Into<#stream_arg<#req_item>> + ::core::marker::Send
                },
                quote!(::core::result::Result<#response, #client_error<#status, #err>>),
                quote!(self.0.client_stream(#path, input).await),
                quote!(C: #client_streaming + #encodes<#req_item> + #decodes<#response>),
            ),

            Capability::BidiStreaming => (
                quote! {
                    &self #extra_params,
                    input: impl ::core::convert::Into<#stream_arg<#req_item>> + ::core::marker::Send
                },
                quote! {
                    ::core::result::Result<
                        <C as #bidi_streaming>::Responses<#resp_item, #err>,
                        #client_error<#status, #err>,
                    >
                },
                quote!(self.0.bidi_stream(#path, input).await),
                quote!(C: #bidi_streaming + #encodes<#req_item> + #decodes<#resp_item>),
            ),
        };

        // Override hints replace the capability-derived pieces for a special call.
        let ret = self.override_ret.clone().unwrap_or(ret);
        let body = self.override_body.clone().unwrap_or(body);
        let bounds = self.override_bounds.clone().unwrap_or(bounds);

        (args, ret, body, bounds)
    }
}

/// Emits a service's client *methods* into a single `impl<C> {Service}Client<C>` block, each
/// method carrying its own `where C: ..` bound (the capability + message codecs, or a hint
/// override). The `{Service}Client<C>` struct itself is emitted once by the component-variant
/// macro (e.g. the RPC `Router`); each impl-block macro contributes methods here, so multiple
/// blocks compose. A method is usable only when `C` satisfies its bound — an unsupported call
/// shape is a bound error at the call site, never a missing method at definition. Emits nothing
/// without the `client` feature or when no methods are contributed.
pub fn generate_client(
    client_ident: &Ident,
    methods: &[ClientMethod],
    paths: &Paths,
) -> TokenStream {
    if !cfg!(feature = "client") || methods.is_empty() {
        return quote!();
    }

    let fns = methods.iter().map(|m| client_method_tokens(m, paths));

    quote! {
        impl<C> #client_ident<C> {
            #(#fns)*
        }
    }
}

/// Renders a single [`ClientMethod`] into its `pub async fn` (signature + `where` bound + body) for
/// an `impl<C> {Service}Client<C>` block. Factored out of [`generate_client`] so a protocol extension
/// can render an *extra* method it constructs (e.g. the axum HTTP `_with_headers` variant) with the
/// exact same capability machinery, keeping the rendering in one place.
pub fn client_method_tokens(m: &ClientMethod, paths: &Paths) -> TokenStream {
    let ident = &m.ident;
    let attrs = &m.attrs;
    let (args, ret, body, bounds) = m.build(paths);

    quote! {
        #(#attrs)*
        #[allow(non_snake_case)]
        pub async fn #ident(#args) -> #ret
        where
            #bounds,
        {
            #body
        }
    }
}

/// The conventional client struct name for a service type, `{Type}Client`.
pub fn client_ident(self_ident: &Ident) -> Ident {
    format_ident!("{}Client", self_ident)
}
