//! The base impl-block macro: `MethodArgs<Ext>` and its expansion state machine.
//!
//! `#[methods]` is `MethodArgs<NoExt>`: it claims `#[init]` constructors, the `#[replace]`,
//! `#[transition]` and `#[hot_update]` transition recipes, and `#[hook]` methods on any
//! component. `#[handlers]` is `MethodArgs<Rpcs>` — the *same* base plus the RPC
//! extension — so handlers also supports init and hooks, and adds RPC registration on top.
//!
//! Expansion is a state machine over the extension ([`ParseKeyed`] → [`ParseItem<ItemImpl>`] →
//! [`ParseMethod`] per method → [`ToTokens`]): the base claims its factory markers and `#[hook]`, delegates
//! every other method to the extension, emits its own output, and appends the extension's.

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use syn::parse::{Parse, ParseStream};
use syn::{Attribute, Ident, ImplItem, ImplItemFn, ItemImpl, LitStr, Type};

use crate::client::ClientMethod;
use crate::extend::{
    NoExt, ParseItem, ParseKeyed, ParseMethod, eat_comma, eat_eq, unknown_key_error,
};
use crate::hook::{self, HookInfo};
use crate::inject::registrations_slice_ident;
use crate::paths::Paths;

mod init;
mod recipe;

#[cfg(test)]
mod tests;

use init::InitInfo;
use recipe::{Recipe, RecipeKind};

/// The base arguments of an impl-block macro: the common `factory_slice` key plus the
/// extension `Ext`, which contributes its own keyed args, per-method parsing, and emission.
/// `MethodArgs<NoExt>` is `#[methods]`; `MethodArgs<Rpcs>` is `#[handlers]`.
#[derive(Default)]
pub struct MethodArgs<Ext = NoExt> {
    factory_slice: Option<Ident>,
    upwell: Option<syn::Path>,
    krate: Option<syn::Path>,
    ext: Ext,
}

impl<Ext: ParseKeyed> MethodArgs<Ext> {
    /// Resolves the crate [`Paths`] for this invocation: the macro's `default` roots with any
    /// `upwell =` / `crate =` overrides applied.
    pub fn paths(&self, default: Paths) -> Paths {
        default.resolve(self.upwell.clone(), self.krate.clone())
    }
}

impl<Ext: ParseKeyed> Parse for MethodArgs<Ext> {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = MethodArgs::<Ext>::default();

        while !input.is_empty() {
            let key: Ident = input.parse()?;

            match key.to_string().as_str() {
                "factory_slice" => {
                    eat_eq(input)?;
                    args.factory_slice = Some(input.parse()?);
                }
                "upwell" => args.upwell = Some(crate::attr::parse_path_override(input)?),
                "crate" => args.krate = Some(crate::attr::parse_path_override(input)?),

                _ => {
                    if !args.ext.parse_keyed(&key, input)? {
                        return Err(unknown_key_error::<Ext>(
                            &key,
                            &["factory_slice", "upwell", "crate"],
                        ));
                    }
                }
            }

            eat_comma(input);
        }

        Ok(args)
    }
}

/// Expands an impl-block macro: runs the extension state machine over `item`, emitting the
/// base output (the `#[init]` factory, transition recipes and `#[hook]` registrations) with the extension's
/// contribution appended.
pub fn expand<Ext>(
    mut args: MethodArgs<Ext>,
    mut item: ItemImpl,
    paths: &Paths,
) -> syn::Result<TokenStream>
where
    Ext: ParseItem<ItemImpl> + ParseMethod + ToTokens,
{
    let self_ty = (*item.self_ty).clone();
    let self_ident = self_ty_ident(&self_ty)?;
    let self_name = LitStr::new(&self_ident.to_string(), self_ident.span());

    // Phase 2: a first pass over the whole impl, so the extension can capture context (the
    // self type, name, and resolved paths) before the per-method walk.
    args.ext.parse_item(&item, paths)?;

    // Walk the methods: the base claims `#[hook]` and the factory markers (stripping them);
    // everything else is offered to the extension's per-method pass, which may return a
    // client-method hint the framework will emit into the generated client.
    let mut factories = FactoryClaims::default();
    let mut hooks: Vec<HookInfo> = Vec::new();
    let mut client_methods: Vec<ClientMethod> = Vec::new();

    for impl_item in &mut item.items {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        if let Some(pos) = method.attrs.iter().position(|a| a.path().is_ident("hook")) {
            let attr = method.attrs.remove(pos);
            let hook_attr = hook::parse_hook_attr(&attr)?;

            hooks.push(hook::parse_hook(method, hook_attr)?);

            continue;
        }

        if factories.claim(method, &self_ident) {
            continue;
        }

        // Phase 3: the extension inspects (and may claim) the method, optionally returning a
        // client-method hint as a byproduct.
        if let Some(client) = args.ext.parse_method(method)? {
            client_methods.push(client);
        }
    }

    if let Some(errors) = factories.errors {
        let errors = errors.into_compile_error();

        return Ok(quote! {
            #errors

            #item
        });
    }

    let registrations_slice = args
        .factory_slice
        .unwrap_or_else(|| registrations_slice_ident(&self_ident));

    let (marker, component) = match &factories.init {
        Some(info) => init::generate_init(&self_ty, &registrations_slice, info, paths),
        None => (quote!(), quote!()),
    };
    let (recipe_wrappers, recipe_registrations): (Vec<_>, Vec<_>) = factories
        .recipes
        .iter()
        .map(|recipe| recipe::generate(&self_ty, &registrations_slice, recipe, paths))
        .unzip();

    let hook_tokens = hooks.iter().enumerate().map(|(index, info)| {
        hook::generate_hook(
            &self_ty,
            &self_name,
            &registrations_slice,
            info,
            index,
            paths,
        )
    });

    // The framework owns the generated (transport-generic) client: emit the capability-partitioned
    // methods from the hints the extension contributed.
    let client_ident = crate::client::client_ident(&self_ident);
    let client = crate::client::generate_client(&client_ident, &client_methods, paths);

    // The extension may emit extra client-side tokens (a target-specific binding, wire-type
    // assertions, …) — emitted ungated after `#client`; the extension owns their content and any
    // gating, so the framework core stays target-agnostic.
    let extra_client = args
        .ext
        .extra_client_tokens(&client_ident, &client_methods, paths);

    // Phase 4: the extension emits its own accumulated contribution, appended after the base.
    let ext = &args.ext;

    // The server surface — the user's `impl` (its handler bodies), the `#[init]` factory, the
    // hooks, and the extension's route table/registration — is gated out on wasm; only the generated
    // client (`#client`) is transport-generic and survives there.
    let server = crate::gate::native_only(quote! {
        #item

        #marker

        #(#recipe_wrappers)*

        const _: () = {
            #component

            #(#recipe_registrations)*

            #(#hook_tokens)*
        };

        #ext
    });

    Ok(quote! {
        #server

        #client

        #extra_client
    })
}

/// A factory marker the base claims from a method.
enum FactoryMarker {
    Init,
    Recipe(RecipeKind),
}

impl FactoryMarker {
    fn of(attr: &Attribute) -> Option<Self> {
        if attr.path().is_ident("init") {
            return Some(Self::Init);
        }

        RecipeKind::ALL
            .into_iter()
            .find(|kind| attr.path().is_ident(kind.marker()))
            .map(Self::Recipe)
    }
}

/// The factory markers an impl block claims, with every diagnostic they raised.
#[derive(Default)]
struct FactoryClaims {
    init: Option<InitInfo>,
    recipes: Vec<Recipe>,
    errors: Option<syn::Error>,
}

impl FactoryClaims {
    /// Claims `method` when it carries a factory marker, stripping every such marker. A
    /// malformed claim is recorded rather than returned, so the walk still strips the rest
    /// and the impl block is kept beside the diagnostics.
    fn claim(&mut self, method: &mut ImplItemFn, self_ident: &Ident) -> bool {
        let mut markers = take_factory_markers(method).into_iter();
        let Some((marker, attr)) = markers.next() else {
            return false;
        };

        if let Some((_, second)) = markers.next() {
            self.reject(syn::Error::new_spanned(
                second,
                "a method carries at most one of `#[init]`, `#[replace]`, `#[transition]` and `#[hot_update]`",
            ));

            return true;
        }

        let claimed = match marker {
            FactoryMarker::Init => self.claim_init(method),
            FactoryMarker::Recipe(kind) => self.claim_recipe(kind, &attr, method, self_ident),
        };

        if let Err(error) = claimed {
            self.reject(error);
        }

        true
    }

    fn claim_init(&mut self, method: &ImplItemFn) -> syn::Result<()> {
        if self.init.is_some() {
            return Err(syn::Error::new_spanned(
                &method.sig,
                "this impl block already has an #[init] constructor",
            ));
        }

        self.init = Some(init::parse_init(method)?);

        Ok(())
    }

    fn claim_recipe(
        &mut self,
        kind: RecipeKind,
        attr: &Attribute,
        method: &ImplItemFn,
        self_ident: &Ident,
    ) -> syn::Result<()> {
        if self.recipes.iter().any(|recipe| recipe.kind == kind) {
            return Err(syn::Error::new_spanned(
                &method.sig,
                format!(
                    "this impl block already has a `#[{}]` recipe",
                    kind.marker()
                ),
            ));
        }

        self.recipes
            .push(recipe::parse(kind, attr, method, self_ident)?);

        Ok(())
    }

    fn reject(&mut self, error: syn::Error) {
        match &mut self.errors {
            Some(errors) => errors.combine(error),

            None => self.errors = Some(error),
        }
    }
}

/// Strips every factory marker from `method`, in declaration order.
fn take_factory_markers(method: &mut ImplItemFn) -> Vec<(FactoryMarker, Attribute)> {
    let mut markers = Vec::new();
    let mut kept = Vec::new();

    for attr in method.attrs.drain(..) {
        match FactoryMarker::of(&attr) {
            Some(marker) => markers.push((marker, attr)),

            None => kept.push(attr),
        }
    }

    method.attrs = kept;

    markers
}

/// Extracts the bare identifier of the impl's self type, erroring on anything but a plain path
/// type (e.g. `Greeter` or `Greeter<T>`). Shared by the impl-block extensions.
pub fn self_ty_ident(ty: &Type) -> syn::Result<syn::Ident> {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.clone())
            .ok_or_else(|| syn::Error::new_spanned(ty, "expected a named type")),

        _ => Err(syn::Error::new_spanned(
            ty,
            "an impl-block macro must be applied to an inherent impl of a named type",
        )),
    }
}
