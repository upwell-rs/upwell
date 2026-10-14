//! Shared codegen for `#[hook(Kind)]` methods, used by `#[methods]` and `#[handlers]`.
//!
//! A hook is an `async` method — optionally taking `&self` — whose parameters are the
//! kind's inputs (each a `HookParam<Kind>`, e.g. `CfgNext<T>` for `ConfigReload`), targeted
//! by an optional per-parameter `#[config("path")]`. Each hook is compiled to a deps
//! reporter and an erased call, registered into the type's `{Type}Hooks` slice. Component
//! dependencies are NOT parameters — a hook reaches them through `&self`.

use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::punctuated::Punctuated;
use syn::{FnArg, Ident, ImplItemFn, LitStr, Meta, Path, Token, Type};

use crate::attr;
use crate::paths::Paths;

/// One hook parameter: its (kind-input) type and optional `#[config("path")]`.
struct HookParamInfo {
    ty: Type,
    path: Option<LitStr>,
}

/// The parsed `#[hook(Kind[, observe][, always])]` attribute.
pub struct HookAttr {
    kind: Path,
    observe: bool,
    always: bool,
}

/// The parsed shape of a `#[hook(Kind)]` method.
pub struct HookInfo {
    ident: syn::Ident,
    attr: HookAttr,
    takes_self: bool,
    params: Vec<HookParamInfo>,
    is_result: bool,
}

/// Parses a `#[hook(kind)]` method, stripping per-parameter `#[config("..")]` attributes so
/// the re-emitted method stays valid. `attr` is the parsed `#[hook(..)]` attribute, already
/// removed by the caller.
pub fn parse_hook(method: &mut ImplItemFn, attr: HookAttr) -> syn::Result<HookInfo> {
    if method.sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            &method.sig,
            "#[hook] methods must be async",
        ));
    }

    let mut takes_self = false;
    let mut params = Vec::new();

    for arg in &mut method.sig.inputs {
        match arg {
            FnArg::Receiver(receiver) => {
                if receiver.reference.is_none() || receiver.mutability.is_some() {
                    return Err(syn::Error::new_spanned(
                        receiver,
                        "a #[hook] receiver must be `&self` (or omit `self` entirely)",
                    ));
                }

                takes_self = true;
            }

            FnArg::Typed(typed) => {
                let path = take_config_path(&mut typed.attrs)?;

                params.push(HookParamInfo {
                    ty: (*typed.ty).clone(),
                    path,
                });
            }
        }
    }

    Ok(HookInfo {
        ident: method.sig.ident.clone(),
        attr,
        takes_self,
        params,
        is_result: attr::result_type_args(&method.sig.output).is_some(),
    })
}

/// Removes a `#[config("path")]` attribute from a parameter, returning its path literal.
fn take_config_path(attrs: &mut Vec<syn::Attribute>) -> syn::Result<Option<LitStr>> {
    let Some(pos) = attrs.iter().position(|a| a.path().is_ident("config")) else {
        return Ok(None);
    };

    let attr = attrs.remove(pos);

    let Meta::List(list) = &attr.meta else {
        return Err(syn::Error::new_spanned(
            &attr,
            "expected #[config(\"path\")]",
        ));
    };

    let path = syn::parse2::<LitStr>(list.tokens.clone())?;

    Ok(Some(path))
}

/// Emits a hook's deps reporter, erased call, and `HookDescriptor` (appended to the type's
/// `{Type}Hooks` slice). `index` disambiguates multiple hooks on one type.
#[allow(clippy::too_many_arguments)]
pub fn generate_hook(
    self_ty: &Type,
    name: &LitStr,
    registrations_slice: &syn::Ident,
    info: &HookInfo,
    index: usize,
    paths: &Paths,
) -> TokenStream {
    let any = quote!(::core::any);
    let hook_kind = paths.core("HookKind");
    let hook_param = paths.core("HookParam");
    let hook_descriptor = paths.core("HookDescriptor");
    let resolver_ctx = paths.core("ResolverCtx");
    let resolver_ctx_ext = paths.core("ResolverCtxExt");
    let component_source = paths.core("ComponentSource");
    let component = paths.core("Component");
    let dependency_descriptor = paths.core("DependencyDescriptor");
    let type_descriptor = paths.core("TypeDescriptor");
    let hook_error = paths.core("HookError");
    let hook_result = paths.core("HookResult");
    let distributed_slice = paths.core("linkme::distributed_slice");
    let linkme_crate = paths.core("linkme");
    let inventory = paths.core("inventory");
    let registration = paths.core("Registration");
    let descriptor_for = paths.core("DescriptorFor");

    let hook_role = paths.core("HookRole");
    let hook_routing = paths.core("HookRouting");

    let kind = &info.attr.kind;
    let method = &info.ident;

    let deps_fn = format_ident!("__upwell_hook_{index}_deps");
    let call_fn = format_ident!("__upwell_hook_{index}_call");
    let kind_ty_fn = format_ident!("__upwell_hook_{index}_kind_ty");
    let descriptor_static = format_ident!("__UPWELL_HOOK_{index}");

    let arg_idents: Vec<_> = (0..info.params.len())
        .map(|i| format_ident!("__a{i}"))
        .collect();
    let param_tys: Vec<&Type> = info.params.iter().map(|p| &p.ty).collect();
    let param_paths: Vec<TokenStream> = info
        .params
        .iter()
        .map(|p| match &p.path {
            Some(lit) => quote!(::core::option::Option::Some(#lit)),
            None => quote!(::core::option::Option::None),
        })
        .collect();

    // Resolve the receiver only when the method takes `&self`; a self-less hook is an
    // associated call. The receiver is reached through the component source resolver fetched
    // from the erased resolver context, so the hook layer never names the container.
    let invoke = if info.takes_self {
        quote! {
            let __svc = #resolver_ctx_ext::get_resolver::<#component_source>(__ctx)
                .and_then(|__src| __src.component::<#self_ty>())
                .ok_or(#hook_error::MissingReceiver(<#self_ty as #component>::NAME))?;
            let __out = __svc.#method(#(#arg_idents),*).await;
        }
    } else {
        quote! {
            let __out = <#self_ty>::#method(#(#arg_idents),*).await;
        }
    };

    // Normalize the return: unwrap a `Result`, boxing its error into the hook layer's error
    // (the hook layer sits below the daemon, so it cannot name the daemon error — any
    // `Error + Send + Sync` domain error boxes), then bind it at the kind's `Output` for a
    // participant or at `()` for an observer, so a mismatched return is a compile error and
    // an observer can never return an action.
    let output = if info.attr.observe {
        quote!(())
    } else {
        quote!(<#kind as #hook_kind>::Output)
    };
    let normalize = if info.is_result {
        quote! {
            let __out: #output = __out.map_err(|__e| #hook_error::Other(__e.into()))?;
        }
    } else {
        quote! {
            let __out: #output = __out;
        }
    };
    let role = info
        .attr
        .observe
        .then(|| quote!(.with_role(#hook_role::Observer)));
    let routing = info
        .attr
        .always
        .then(|| quote!(.with_routing(#hook_routing::Always)));

    // Source-position ordinal: `line!()` resolved at the hook method's span, so a type's hooks run
    // in source order on the `inventory` backend (which does not preserve registration order) just
    // as they do on `linkme`.
    let ordinal = quote_spanned!(method.span()=> ::core::line!());
    let hook_literal = quote! {
        #hook_descriptor::new(
            #ordinal,
            #type_descriptor::of::<#self_ty>(#name),
            <#kind as #hook_kind>::NAME,
            #kind_ty_fn,
            #deps_fn,
            #call_fn,
        )
        #role
        #routing
    };
    // The hook, appended to the type's registrations.
    let register = crate::backend::dual_backend(
        quote! {
            #inventory::submit! {
                #descriptor_for::<#self_ty, #hook_descriptor>::new(#hook_literal)
            }
        },
        quote! {
            #[#distributed_slice(#registrations_slice)]
            #[linkme(crate = #linkme_crate)]
            #[allow(unsafe_code)]
            static #descriptor_static: #registration = #registration::Hook(#hook_literal);
        },
    );

    quote! {
        fn #deps_fn() -> ::std::vec::Vec<#dependency_descriptor> {
            ::std::vec![
                #( <#param_tys as #hook_param<#kind>>::dependency(#param_paths) ),*
            ]
        }

        fn #kind_ty_fn() -> #any::TypeId {
            #any::TypeId::of::<#kind>()
        }

        #[allow(unused_variables)]
        fn #call_fn<'a>(
            __ctx: &'a (dyn #resolver_ctx + ::core::marker::Send + ::core::marker::Sync),
            __cx: &'a (dyn #any::Any + ::core::marker::Send + ::core::marker::Sync),
        ) -> ::core::pin::Pin<
            ::std::boxed::Box<
                dyn ::core::future::Future<
                    Output = #hook_result<::std::boxed::Box<dyn #any::Any + ::core::marker::Send>>,
                > + ::core::marker::Send + 'a,
            >,
        > {
            ::std::boxed::Box::pin(async move {
                let __cx = __cx
                    .downcast_ref::<<#kind as #hook_kind>::Cx>()
                    .expect("hook context matches its kind");

                #(
                    let #arg_idents =
                        <#param_tys as #hook_param<#kind>>::extract(__cx, #param_paths)?;
                )*

                #invoke

                #normalize

                ::core::result::Result::Ok(
                    ::std::boxed::Box::new(__out) as ::std::boxed::Box<dyn #any::Any + ::core::marker::Send>
                )
            })
        }

        #register
    }
}

/// Parses a `#[hook(Kind[, observe][, always])]` attribute: the kind path, then optional
/// flags declaring an observer role and untargeted routing, each at most once.
pub fn parse_hook_attr(attr: &syn::Attribute) -> syn::Result<HookAttr> {
    let Meta::List(list) = &attr.meta else {
        return Err(syn::Error::new_spanned(
            attr,
            "expected #[hook(Kind)] naming a hook kind type",
        ));
    };

    let (kind, flags) = list.parse_args_with(|input: syn::parse::ParseStream| {
        let kind = input.parse::<Path>()?;
        let mut flags = Punctuated::<Ident, Token![,]>::new();

        if input.parse::<Option<Token![,]>>()?.is_some() {
            flags = Punctuated::parse_terminated(input)?;
        }

        Ok((kind, flags))
    })?;
    let mut parsed = HookAttr {
        kind,
        observe: false,
        always: false,
    };

    for flag in flags {
        let slot = match flag.to_string().as_str() {
            "observe" => &mut parsed.observe,
            "always" => &mut parsed.always,
            _ => {
                return Err(syn::Error::new_spanned(
                    flag,
                    "unknown hook flag: expected `observe` or `always`",
                ));
            }
        };

        if *slot {
            return Err(syn::Error::new_spanned(flag, "duplicate hook flag"));
        }

        *slot = true;
    }

    Ok(parsed)
}
