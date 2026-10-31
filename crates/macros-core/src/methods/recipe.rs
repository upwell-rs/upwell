//! Transition recipes declared in an impl block: `#[replace]`, `#[transition]` and
//! `#[hot_update]` methods.
//!
//! Each expands to the `ComponentFactoryDescriptor` the `factories!` builder declares for
//! `replace`, `transition` and `hot_update`, under the ID `"<marker>:<method>"`. A fixed-name
//! hidden wrapper per kind normalizes the method to the shape the descriptor drives (async
//! for a replacement or hot update) and guards against a second recipe of the same kind in
//! another impl block (E0592).

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{
    Attribute, FnArg, GenericArgument, Ident, ImplItemFn, LitStr, PathArguments, ReturnType, Type,
};

use crate::inject::register_factory;
use crate::paths::Paths;

/// A transition-recipe marker an impl-block macro claims.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RecipeKind {
    Replace,
    Transition,
    HotUpdate,
}

/// One parsed recipe method.
pub(super) struct Recipe {
    pub(super) kind: RecipeKind,
    marker: Span,
    ident: Ident,
    is_async: bool,
    param_types: Vec<Type>,
    output: ReturnType,
}

impl RecipeKind {
    pub(super) const ALL: [Self; 3] = [Self::Replace, Self::Transition, Self::HotUpdate];

    /// The marker attribute's name.
    pub(super) fn marker(self) -> &'static str {
        match self {
            Self::Replace => "replace",
            Self::Transition => "transition",
            Self::HotUpdate => "hot_update",
        }
    }

    fn role(self) -> Ident {
        let role = match self {
            Self::Replace => "Replacement",
            Self::Transition => "Transition",
            Self::HotUpdate => "HotUpdate",
        };

        Ident::new(role, Span::call_site())
    }

    /// The fixed-name wrapper, spanned at the marked method so a clash points there.
    fn guard(self, method: &Ident) -> Ident {
        Ident::new(&format!("__upwell_{}", self.marker()), method.span())
    }

    fn registration(self) -> Ident {
        format_ident!("__UPWELL_{}_RECIPE", self.marker().to_uppercase())
    }

    /// What the `Previous<Self>` first parameter is to this kind, if it takes one.
    fn previous_role(self) -> Option<&'static str> {
        match self {
            Self::Replace => Some("the active instance it replaces"),
            Self::HotUpdate => Some("the active instance it updates"),
            Self::Transition => None,
        }
    }
}

/// Parses a method marked `attr` as a `kind` recipe of `self_ident`.
pub(super) fn parse(
    kind: RecipeKind,
    attr: &Attribute,
    method: &ImplItemFn,
    self_ident: &Ident,
) -> syn::Result<Recipe> {
    let marker = kind.marker();
    let mut param_types = Vec::new();

    if !method.sig.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &method.sig.generics,
            format!(
                "a `#[{marker}]` recipe cannot be generic; its descriptor drives one concrete function"
            ),
        ));
    }

    if !matches!(attr.meta, syn::Meta::Path(_)) {
        return Err(syn::Error::new_spanned(
            &attr.meta,
            format!("`#[{marker}]` takes no arguments"),
        ));
    }

    for arg in &method.sig.inputs {
        match arg {
            FnArg::Receiver(receiver) => {
                return Err(syn::Error::new_spanned(
                    receiver,
                    format!(
                        "a `#[{marker}]` recipe is an associated function and cannot take `self`"
                    ),
                ));
            }

            FnArg::Typed(typed) => param_types.push((*typed.ty).clone()),
        }
    }

    match kind.previous_role() {
        Some(role) => require_previous(marker, role, method, param_types.first(), self_ident)?,

        None => require_handler_shape(method, param_types.len())?,
    }

    Ok(Recipe {
        kind,
        marker: attr.path().span(),
        ident: method.sig.ident.clone(),
        is_async: method.sig.asyncness.is_some(),
        param_types,
        output: method.sig.output.clone(),
    })
}

/// Requires a replacement or hot update to read the active instance first.
fn require_previous(
    marker: &str,
    role: &str,
    method: &ImplItemFn,
    first: Option<&Type>,
    self_ident: &Ident,
) -> syn::Result<()> {
    let message =
        format!("the first parameter of a `#[{marker}]` recipe must be `Previous<Self>`, {role}");

    match first {
        Some(ty) if is_previous_of_self(ty, self_ident) => Ok(()),

        Some(ty) => Err(syn::Error::new_spanned(ty, message)),

        None => Err(syn::Error::new_spanned(&method.sig, message)),
    }
}

/// Requires a transition handler to be a plain `fn(&ComponentChange) -> ..`.
fn require_handler_shape(method: &ImplItemFn, params: usize) -> syn::Result<()> {
    if let Some(asyncness) = &method.sig.asyncness {
        return Err(syn::Error::new_spanned(
            asyncness,
            "a `#[transition]` handler only selects a factory and cannot be `async`; do async work in the `OneShot` factory it returns",
        ));
    }

    if params != 1 {
        return Err(syn::Error::new_spanned(
            &method.sig,
            "a `#[transition]` handler takes exactly one parameter: `change: &ComponentChange`",
        ));
    }

    Ok(())
}

fn is_previous_of_self(ty: &Type, self_ident: &Ident) -> bool {
    let Type::Path(path) = ty else {
        return false;
    };
    let Some(segment) = path.path.segments.last() else {
        return false;
    };
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return false;
    };

    if segment.ident != "Previous" || arguments.args.len() != 1 {
        return false;
    }

    match arguments.args.first() {
        Some(GenericArgument::Type(Type::Path(inner))) => {
            inner.qself.is_none()
                && (inner.path.is_ident("Self") || inner.path.is_ident(self_ident))
        }

        _ => false,
    }
}

/// Emits `recipe`: its hidden wrapper on `self_ty`, and the singleton-scope assertion plus
/// descriptor registration for the impl block's `const _` block.
pub(super) fn generate(
    self_ty: &Type,
    registrations_slice: &Ident,
    recipe: &Recipe,
    paths: &Paths,
) -> (TokenStream, TokenStream) {
    let guard = recipe.kind.guard(&recipe.ident);
    let factory = quote_spanned!(recipe.ident.span()=> <#self_ty>::#guard);
    let wrapper = wrapper(self_ty, recipe, paths);
    let literal = descriptor(recipe, &factory, paths);
    let register = register_factory(
        self_ty,
        registrations_slice,
        &recipe.kind.registration(),
        &literal,
        paths,
    );
    let assertion = singleton_assertion(self_ty, recipe, paths);

    (
        wrapper,
        quote! {
            #assertion

            #register
        },
    )
}

/// The fixed-name hidden associated fn the descriptor drives: an async forwarder for a
/// replacement or hot update, a typed forwarder for a transition handler.
fn wrapper(self_ty: &Type, recipe: &Recipe, paths: &Paths) -> TokenStream {
    let guard = recipe.kind.guard(&recipe.ident);
    let marked = &recipe.ident;

    if recipe.kind == RecipeKind::Transition {
        let component_change = paths.core("ComponentChange");
        let one_shot = paths.core("OneShot");

        return quote_spanned! {marked.span()=>
            impl #self_ty {
                #[doc(hidden)]
                fn #guard(
                    change: &#component_change,
                ) -> ::core::option::Option<#one_shot<#self_ty>> {
                    <#self_ty>::#marked(change)
                }
            }
        };
    }

    let fresh: Vec<_> = (0..recipe.param_types.len())
        .map(|i| format_ident!("__p{i}"))
        .collect();
    let param_types = &recipe.param_types;
    let output = &recipe.output;
    let dotawait = if recipe.is_async {
        quote!(.await)
    } else {
        quote!()
    };

    quote_spanned! {marked.span()=>
        impl #self_ty {
            #[doc(hidden)]
            async fn #guard(#(#fresh: #param_types),*) #output {
                <#self_ty>::#marked(#(#fresh),*)#dotawait
            }
        }
    }
}

/// The `ComponentFactoryDescriptor` literal, identical to the `factories!` builder entry.
fn descriptor(recipe: &Recipe, factory: &TokenStream, paths: &Paths) -> TokenStream {
    let component_factory_descriptor = paths.core("ComponentFactoryDescriptor");
    let component_factory_role = paths.core("ComponentFactoryRole");
    let role = recipe.kind.role();
    let span = recipe.ident.span();
    let id = LitStr::new(&format!("{}:{}", recipe.kind.marker(), recipe.ident), span);
    let (dispatch, dependencies) = match recipe.kind {
        RecipeKind::Replace => {
            let dispatch = paths.core("dispatch_replacement");
            let dependencies = paths.core("replacement_dependencies");

            (dispatch, quote_spanned!(span=> || #dependencies(#factory)))
        }

        RecipeKind::Transition => (
            paths.core("dispatch_transition"),
            quote!(::std::vec::Vec::new),
        ),

        RecipeKind::HotUpdate => {
            let dispatch = paths.core("dispatch_hot_update");
            let dependencies = paths.core("hot_update_dependencies");

            (dispatch, quote_spanned!(span=> || #dependencies(#factory)))
        }
    };

    let construct = quote_spanned!(span=> |cx| #dispatch(#factory, cx));

    quote! {
        #component_factory_descriptor {
            id: #id,
            construct: #construct,
            construct_ready: ::core::option::Option::None,
            dependencies: #dependencies,
            role: #component_factory_role::#role,
        }
    }
}

/// A `const` assertion, failing at the marker, that the component's declared scope is the
/// singleton scope.
fn singleton_assertion(self_ty: &Type, recipe: &Recipe, paths: &Paths) -> TokenStream {
    let declared_scope = paths.core("DeclaredScope");
    let static_scope = paths.core("Scope");
    let singleton = paths.core("scope::Singleton");
    let message = LitStr::new(
        &format!(
            "`#[{}]` requires a singleton component; runtime transitions are singleton-only",
            recipe.kind.marker()
        ),
        recipe.marker,
    );

    quote_spanned! {recipe.marker=>
        const _: () = ::core::assert!(
            <<#self_ty as #declared_scope>::Scope as #static_scope>::RANK
                == <#singleton as #static_scope>::RANK,
            #message,
        );
    }
}
