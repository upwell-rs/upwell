//! The `#[init]` constructor: an explicit factory that overrides the field-injection default.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, ImplItemFn, ReturnType, Type};

use crate::inject::register_factory;
use crate::paths::Paths;

/// The parsed shape of an `#[init]` constructor.
pub(super) struct InitInfo {
    ident: syn::Ident,
    is_async: bool,
    param_types: Vec<Type>,
    output: ReturnType,
}

pub(super) fn parse_init(method: &ImplItemFn) -> syn::Result<InitInfo> {
    let mut param_types = Vec::new();

    // Parameter *types* only — each must be a `FromContainer` (`Arc<T>`, `Cfg<T>`,
    // `Vec<Arc<dyn Tr>>`, a by-value injectable, …), enforced by the `Factory` impl
    // at the type level rather than by shape-matching here.
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
pub(super) fn generate_init(
    self_ty: &Type,
    registrations_slice: &syn::Ident,
    info: &InitInfo,
    paths: &Paths,
) -> (TokenStream, TokenStream) {
    let marked = &info.ident;
    let component_construction_context = paths.core("ComponentConstructionContext");
    let component_factory_descriptor = paths.core("ComponentFactoryDescriptor");
    let component_factory_role = paths.core("ComponentFactoryRole");
    let dependency_descriptor = paths.core("DependencyDescriptor");
    let dispatch_factory = paths.core("dispatch_factory");
    let factory_dependencies = paths.core("factory_dependencies");
    let boxed_component = paths.core("BoxedComponent");
    let result = paths.core("DiResult");

    // A fixed-name `init` associated fn, always `async`, forwards to the marked constructor —
    // normalizing a sync constructor to async (factories are async) and serving as the
    // compile-time uniqueness guard: two `#[init]`s in one impl emit two `fn init` and fail
    // with E0592. When the marked method is already named `init`, it is its own guard and
    // must itself be `async`.
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

    let factory_literal = quote! {
        #component_factory_descriptor {
            id: concat!("init:", stringify!(#marked)),
            construct: __upwell_init_factory,
            dependencies: __upwell_init_deps,
            role: #component_factory_role::Explicit,
        }
    };
    // The explicit `#[init]` factory, appended to the type's registrations; it overrides the
    // field-injection default.
    let register = register_factory(
        self_ty,
        registrations_slice,
        &format_ident!("__UPWELL_INIT_FACTORY"),
        &factory_literal,
        paths,
    );

    // The constructor is a `Factory`: it knows its parameters, so it reports its own
    // dependency edges and drives construction. No hand-built dep list — each parameter's
    // `FromContainer` impl supplies its edge.
    let component = quote! {
        fn __upwell_init_deps() -> ::std::vec::Vec<#dependency_descriptor> {
            #factory_dependencies(<#self_ty>::init)
        }

        #[allow(unused_variables)]
        fn __upwell_init_factory(
            cx: &mut #component_construction_context,
        ) -> ::core::pin::Pin<
            ::std::boxed::Box<
                dyn ::core::future::Future<
                    Output = #result<#boxed_component>,
                > + ::core::marker::Send + '_,
            >,
        > {
            #dispatch_factory(<#self_ty>::init, cx)
        }

        #register
    };

    (marker, component)
}
