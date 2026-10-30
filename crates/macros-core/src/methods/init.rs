//! The `#[init]` constructor: an explicit factory that overrides the field-injection default.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, ImplItemFn, ReturnType, Type};

use crate::inject::{factory_descriptor, register_factory};
use crate::paths::Paths;

/// The parsed shape of an `#[init]` constructor.
pub(super) struct InitInfo {
    ident: syn::Ident,
    is_async: bool,
    param_types: Vec<Type>,
    output: ReturnType,
}

/// Parses an `#[init]` constructor's shape. Only parameter *types* are recorded: each must
/// be a `FromContainer` (`Arc<T>`, `Cfg<T>`, `Vec<Arc<dyn Tr>>`, a by-value injectable, …),
/// which the `Factory` impl enforces at the type level rather than by shape-matching here.
pub(super) fn parse_init(method: &ImplItemFn) -> syn::Result<InitInfo> {
    let mut param_types = Vec::new();

    for arg in &method.sig.inputs {
        match arg {
            FnArg::Receiver(receiver) => {
                return Err(syn::Error::new_spanned(
                    receiver,
                    "#[init] is a constructor and cannot take `self`",
                ));
            }
            FnArg::Typed(typed) => {
                param_types.push((*typed.ty).clone());
            }
        }
    }

    Ok(InitInfo {
        ident: method.sig.ident.clone(),
        is_async: method.sig.asyncness.is_some(),
        param_types,
        output: method.sig.output.clone(),
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

    let fresh: Vec<_> = (0..info.param_types.len())
        .map(|i| format_ident!("__p{i}"))
        .collect();
    let param_types = &info.param_types;
    let output = &info.output;
    let dotawait = if info.is_async {
        quote!(.await)
    } else {
        quote!()
    };

    let marker = if marked == "init" {
        quote!()
    } else {
        quote! {
            impl #self_ty {
                #[doc(hidden)]
                async fn init(#(#fresh: #param_types),*) #output {
                    <#self_ty>::#marked(#(#fresh),*)#dotawait
                }
            }
        }
    };

    let ready = (!info.is_async).then(|| quote!(<#self_ty>::#marked));
    let factory = factory_descriptor(
        &quote!(concat!("init:", stringify!(#marked))),
        &quote!(<#self_ty>::init),
        ready.as_ref(),
        paths,
    );
    let component = register_factory(
        self_ty,
        registrations_slice,
        &format_ident!("__UPWELL_INIT_FACTORY"),
        &factory,
        paths,
    );

    (marker, component)
}
