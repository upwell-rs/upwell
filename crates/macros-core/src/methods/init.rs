//! The `#[init]` constructor: an explicit factory that overrides the field-injection default.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::{Attribute, FnArg, ImplItemFn, ReturnType, Token, Type};

use super::init_params::InitParam;
use crate::inject::{factory_descriptor, register_factory};
use crate::paths::Paths;

/// The parsed shape of an `#[init]` constructor.
pub(super) struct InitInfo {
    ident: syn::Ident,
    is_async: bool,
    params: Vec<InitParam>,
    output: ReturnType,
    scope: Option<syn::Path>,
}

/// The arguments of `#[init(..)]`: only `scope = <ScopePath>`, spelled as on `#[component]`.
#[derive(Default)]
struct InitArgs {
    scope: Option<syn::Path>,
}

impl Parse for InitArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = Self::default();

        while !input.is_empty() {
            let key: syn::Ident = input.parse()?;

            if key != "scope" {
                return Err(syn::Error::new(
                    key.span(),
                    "unknown `#[init]` argument; the only argument is `scope = <ScopePath>`",
                ));
            }

            if args.scope.is_some() {
                return Err(syn::Error::new(key.span(), "duplicate `scope` argument"));
            }

            input.parse::<Token![=]>()?;
            args.scope = Some(crate::attr::parse_scope_path(input)?);

            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(args)
    }
}

/// Parses an `#[init]` constructor's shape. Only parameter *types* are recorded: each must
/// be a `FromContainer` (`Arc<T>`, `Cfg<T>`, `Vec<Arc<dyn Tr>>`, a by-value injectable, …),
/// which the `Factory` impl enforces at the type level rather than by shape-matching here.
pub(super) fn parse_init(attr: &Attribute, method: &mut ImplItemFn) -> syn::Result<InitInfo> {
    let mut params = Vec::new();
    let args = match &attr.meta {
        syn::Meta::Path(_) => InitArgs::default(),
        syn::Meta::List(list) => list.parse_args::<InitArgs>()?,
        syn::Meta::NameValue(value) => {
            return Err(syn::Error::new_spanned(
                value,
                "`#[init]` takes `scope = <ScopePath>` in parentheses",
            ));
        }
    };

    for arg in &mut method.sig.inputs {
        match arg {
            FnArg::Receiver(receiver) => {
                return Err(syn::Error::new_spanned(
                    receiver,
                    "#[init] is a constructor and cannot take `self`",
                ));
            }
            FnArg::Typed(typed) => {
                params.push(InitParam::take(typed)?);
            }
        }
    }

    Ok(InitInfo {
        ident: method.sig.ident.clone(),
        is_async: method.sig.asyncness.is_some(),
        params,
        output: method.sig.output.clone(),
        scope: args.scope,
    })
}

/// Emits the `#[init]` factory: a fixed-name async `init` wrapper (the guard and sync→async
/// normalizer) plus its `ComponentFactoryDescriptor`, appended to the type's factory slice.
/// A sync constructor is also the descriptor's ready factory, so it builds without a future
/// when every parameter is ready.
///
/// The fixed-name `init` associated fn is always `async` and forwards to the marked
/// constructor, normalizing a sync constructor (factories are async) and serving as the
/// compile-time uniqueness guard: two `#[init]`s in one impl emit two `fn init` and fail
/// with E0592. When the marked method is already named `init`, it is its own guard and must
/// itself be `async`.
pub(super) fn generate_init(
    self_ty: &Type,
    registrations_slice: &syn::Ident,
    info: &InitInfo,
    paths: &Paths,
) -> (TokenStream, TokenStream) {
    let marked = &info.ident;

    let fresh: Vec<_> = (0..info.params.len())
        .map(|i| format_ident!("__p{i}"))
        .collect();
    let wrappers: Vec<_> = (0..info.params.len())
        .map(|i| format_ident!("__Param{i}"))
        .collect();
    let output = &info.output;
    let dotawait = if info.is_async {
        quote!(.await)
    } else {
        quote!()
    };

    let bound = info.params.iter().any(InitParam::is_bound);
    let named = info.scope.is_some() || bound;

    let param_types: Vec<_> = info
        .params
        .iter()
        .zip(&wrappers)
        .map(|(param, wrapper)| match param.is_bound() {
            true => quote!(#wrapper),
            false => {
                let ty = &param.ty;

                quote!(#ty)
            }
        })
        .collect();
    let arguments: Vec<_> = info
        .params
        .iter()
        .zip(&fresh)
        .map(|(param, name)| match param.is_bound() {
            true => quote!(#name.0),
            false => quote!(#name),
        })
        .collect();
    let wrapper_types = info
        .params
        .iter()
        .zip(&wrappers)
        .map(|(param, wrapper)| param.wrapper(wrapper, paths));

    let wrapper = match named {
        true => format_ident!("__upwell_init_{}", marked),
        false => format_ident!("init"),
    };

    let marker = if marked == "init" && !named {
        quote!()
    } else {
        quote! {
            const _: () = {
                #(#wrapper_types)*

                impl #self_ty {
                    #[doc(hidden)]
                    async fn #wrapper(#(#fresh: #param_types),*) #output {
                        <#self_ty>::#marked(#(#arguments),*)#dotawait
                    }
                }
            };
        }
    };

    let slot = match named {
        true => format_ident!(
            "__UPWELL_INIT_FACTORY_{}",
            marked.to_string().to_uppercase()
        ),
        false => format_ident!("__UPWELL_INIT_FACTORY"),
    };

    let ready = (!info.is_async && !bound).then(|| quote!(<#self_ty>::#marked));
    let factory = factory_descriptor(
        &quote!(concat!("init:", stringify!(#marked))),
        &quote!(<#self_ty>::#wrapper),
        ready.as_ref(),
        info.scope.as_ref(),
        paths,
    );
    let component = register_factory(self_ty, registrations_slice, &slot, &factory, paths);

    (marker, component)
}
