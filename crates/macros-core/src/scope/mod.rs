//! `#[scope]` expansion: declares a struct as a scope whose fields are each instance's
//! parameters.
//!
//! The struct is kept unchanged. Beside it the macro emits, from the one `parents` list, the
//! `Scope` impl, an `OpensUnder<P>` impl and a rank check per parent, from the `children` list
//! an `impl OpensUnder<Scope> for Child` and a rank check per child, and the scope's
//! `ScopeDeclaration`, carried by the type (so the builder can register it explicitly) and
//! appended to the `SCOPES` discovery slice. Preparation adds the declaration's boundary to the
//! protocol's topology and its parameter slot to the registry, so the trait impls and the
//! topology cannot drift apart. That generated surface is server-only and gated out on wasm.

mod accessors;
mod args;

use std::mem;

use heck::ToSnakeCase;
use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::ext::IdentExt;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Fields, ItemStruct, LitStr, Token};

pub use args::ScopeArgs;

use crate::paths::Paths;
use args::Generations;

/// The namespace of every scope ID `#[scope]` derives from its type name.
const DEFAULT_NAMESPACE: &str = "app";

/// Expands `#[scope]` on `item`.
pub fn expand(args: ScopeArgs, mut item: ItemStruct, paths: &Paths) -> syn::Result<TokenStream> {
    reject_generics(&item)?;

    let ident = &item.ident;
    let id = match &args.id {
        Some(id) => id.clone(),
        None => default_id(ident)?,
    };
    let name = match &args.name {
        Some(name) => name.clone(),
        None => LitStr::new(&ident.unraw().to_string(), ident.span()),
    };
    let fields = field_names(&item.fields);
    let log_params = take_log_params(&mut item.fields, paths)?;

    let scope_trait = paths.core("Scope");
    let scope_id = paths.core("ScopeId");
    let namespaced_id = paths.core("namespaced_id");
    let descriptor_trait = paths.core("Descriptor");
    let declaration = paths.core("ScopeDeclaration");
    let scope_parent = paths.core("ScopeParent");
    let scopes = paths.core("SCOPES");
    let distributed_slice = paths.core("linkme::distributed_slice");
    let linkme_crate = paths.core("linkme");

    let rank = rank(&args, paths);
    let generations = generations(args.generations.as_ref(), paths);
    let opens_under = opens_under(&args.parents, ident, paths);
    let rank_checks = rank_checks(&args.parents, ident, paths);
    let children = children(&args.children, ident, paths);
    let child_ids = args.children.iter().map(|child| {
        let scope_trait = paths.core("Scope");

        quote_spanned!(child.span()=> <#child as #scope_trait>::ID)
    });
    let parents = args
        .parents
        .iter()
        .map(|parent| quote_spanned!(parent.span()=> #scope_parent::of::<#parent>()));
    let source = crate::source::item_source(ident.span(), &ident.unraw().to_string(), paths);
    let provided = provided(&item.fields, ident, paths);
    let accessors = accessors::accessors(&item.fields, ident, paths);

    let server = crate::gate::native_only(quote! {
        impl #scope_trait for #ident {
            const ID: #scope_id = #namespaced_id!(#scope_id, #id);
            const RANK: u8 = #rank;
            const NAME: &'static str = #name;
            #generations
            #log_params
        }

        #opens_under

        #rank_checks

        #children

        const _: () = ::core::assert!(
            <#ident as #scope_trait>::RANK >= 1,
            "a #[scope] rank must be at least 1: rank 0 belongs to the transient scope"
        );

        impl #descriptor_trait<#declaration> for #ident {
            const DESCRIPTOR: #declaration = {
                const PARENTS: &[#scope_parent] = &[#(#parents),*];

                #declaration::new::<#ident>(PARENTS)
                    .with_children(&[#(#child_ids),*])
                    .with_fields(&[#(#fields),*])
                    .declared_at(#source)
            };
        }

        const _: () = {
            #[#distributed_slice(#scopes)]
            #[linkme(crate = #linkme_crate)]
            #[allow(unsafe_code)]
            static __UPWELL_SCOPE: #declaration =
                <#ident as #descriptor_trait<#declaration>>::DESCRIPTOR;
        };

        #provided

        #accessors
    });

    Ok(quote! {
        #[cfg_attr(target_family = "wasm", allow(dead_code))]
        #item

        #server
    })
}

/// Strips the `#[log]` marker from every field that carries it and returns the
/// `record_log_params` method reporting those fields, or nothing when none opts in.
fn take_log_params(fields: &mut Fields, paths: &Paths) -> syn::Result<TokenStream> {
    let mut logged = Vec::new();

    reject_cfg_in_logged_tuple(fields)?;

    for (index, field) in fields.iter_mut().enumerate() {
        let (markers, rest): (Vec<syn::Attribute>, Vec<syn::Attribute>) =
            mem::take(&mut field.attrs)
                .into_iter()
                .partition(|attr| attr.path().is_ident("log"));

        field.attrs = rest;

        let Some(marker) = markers.first() else {
            continue;
        };

        if let Some(extra) = markers.get(1) {
            return Err(syn::Error::new_spanned(
                extra,
                "a field takes at most one `#[log]` marker",
            ));
        }

        if !matches!(marker.meta, syn::Meta::Path(_)) {
            return Err(syn::Error::new_spanned(
                &marker.meta,
                "`#[log]` takes no arguments",
            ));
        }

        let (name, member) = match &field.ident {
            Some(ident) => (
                LitStr::new(&ident.unraw().to_string(), ident.span()),
                syn::Member::Named(ident.clone()),
            ),
            None => (
                LitStr::new(&index.to_string(), field.ty.span()),
                syn::Member::Unnamed(syn::Index {
                    index: index as u32,
                    span: field.ty.span(),
                }),
            ),
        };

        let cfg = effective_cfg(&field.attrs)?.map(|condition| quote!(#[cfg(#condition)]));

        logged.push(quote_spanned!(field.ty.span()=> #cfg params.param(#name, &self.#member);));
    }

    if logged.is_empty() {
        return Ok(quote!());
    }

    let log_params = paths.core("LogParams");

    Ok(quote! {
        fn record_log_params(&self, params: &mut dyn #log_params) {
            #(#logged)*
        }
    })
}

/// Rejects a field attribute that can remove a tuple scope's field when one field is logged: a
/// removed field shifts the positions of the fields after it, so the generated `self.N` would
/// read the wrong field.
fn reject_cfg_in_logged_tuple(fields: &Fields) -> syn::Result<()> {
    let Fields::Unnamed(tuple) = fields else {
        return Ok(());
    };
    let has_marker = tuple
        .unnamed
        .iter()
        .any(|field| field.attrs.iter().any(|attr| attr.path().is_ident("log")));

    if !has_marker {
        return Ok(());
    }

    for field in &tuple.unnamed {
        for attr in &field.attrs {
            if cfg_condition(&attr.meta)?.is_some() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "`#[cfg]` or a `#[cfg_attr]` that can remove a field is not supported on the \
                     fields of a tuple #[scope] with a `#[log]` field: a removed field would \
                     shift the position of the logged one",
                ));
            }
        }
    }

    Ok(())
}

/// The `cfg` predicate under which `attrs` keep their field, or `None` when none can remove it.
/// A `cfg_attr(predicate, ..)` removes the field when `predicate` holds and a `cfg` it carries
/// does not.
fn effective_cfg(attrs: &[syn::Attribute]) -> syn::Result<Option<TokenStream>> {
    let mut conditions = Vec::new();

    for attr in attrs {
        if let Some(condition) = cfg_condition(&attr.meta)? {
            conditions.push(condition);
        }
    }

    if conditions.is_empty() {
        return Ok(None);
    }

    Ok(Some(quote!(all(#(#conditions),*))))
}

/// The predicate under which one attribute keeps its field, recursing through `cfg_attr`.
fn cfg_condition(meta: &syn::Meta) -> syn::Result<Option<TokenStream>> {
    let syn::Meta::List(list) = meta else {
        return Ok(None);
    };

    if list.path.is_ident("cfg") {
        return Ok(Some(list.tokens.clone()));
    }

    if !list.path.is_ident("cfg_attr") {
        return Ok(None);
    }

    let mut args = list
        .parse_args_with(Punctuated::<syn::Meta, Token![,]>::parse_terminated)?
        .into_iter();
    let Some(predicate) = args.next() else {
        return Ok(None);
    };
    let mut nested = Vec::new();

    for arg in args {
        if let Some(condition) = cfg_condition(&arg)? {
            nested.push(condition);
        }
    }

    if nested.is_empty() {
        return Ok(None);
    }

    Ok(Some(quote!(any(not(#predicate), all(#(#nested),*)))))
}

/// Rejects a generic scope: a scope's identity and rank are constants of one type.
fn reject_generics(item: &ItemStruct) -> syn::Result<()> {
    if item.generics.params.is_empty() {
        return Ok(());
    }

    Err(syn::Error::new_spanned(
        &item.generics,
        "a #[scope] type cannot be generic: its identity, rank and parents are constants of \
         one type",
    ))
}

/// `app/<type name in snake_case>`, spanned at the type name.
fn default_id(ident: &syn::Ident) -> syn::Result<LitStr> {
    let name = ident.unraw().to_string();
    let segment = name.to_snake_case();

    if !crate::entry::is_segment(&segment) {
        return Err(syn::Error::new_spanned(
            ident,
            format!(
                "`{name}` has no snake_case ASCII form to derive a scope ID from; name the \
                 scope with `id = \"namespace/name\"`"
            ),
        ));
    }

    Ok(LitStr::new(
        &format!("{DEFAULT_NAMESPACE}/{segment}"),
        ident.span(),
    ))
}

/// The parameter field names, in declaration order: named fields by name, tuple fields by
/// position.
fn field_names(fields: &Fields) -> Vec<LitStr> {
    fields
        .iter()
        .enumerate()
        .map(|(index, field)| match &field.ident {
            Some(ident) => LitStr::new(&ident.unraw().to_string(), ident.span()),
            None => LitStr::new(&index.to_string(), field.ty.span()),
        })
        .collect()
}

/// The explicit rank, or one below the lowest-ranked parent.
fn rank(args: &ScopeArgs, paths: &Paths) -> TokenStream {
    if let Some(rank) = &args.rank {
        return quote!(#rank);
    }

    let scope_trait = paths.core("Scope");
    let rank_below = paths.core("rank_below");
    let parents = args
        .parents
        .iter()
        .map(|parent| quote_spanned!(parent.span()=> <#parent as #scope_trait>::RANK));

    quote!(#rank_below(&[#(#parents),*]))
}

/// The `GENERATIONS` constant, unless the scope keeps the pinned default.
fn generations(generations: Option<&Generations>, paths: &Paths) -> TokenStream {
    let ty = paths.core("ScopeGenerations");
    let value = match generations {
        None => return quote!(),
        Some(Generations::Path(path)) => quote!(#path),
        Some(Generations::Keyword(keyword)) if keyword == "durable" => {
            quote_spanned!(keyword.span()=> #ty::Durable)
        }
        Some(Generations::Keyword(keyword)) => quote_spanned!(keyword.span()=> #ty::Pinned),
    };

    quote!(const GENERATIONS: #ty = #value;)
}

/// `impl OpensUnder<P> for S {}` per parent, spanned at the parent.
fn opens_under(parents: &[syn::Path], ident: &syn::Ident, paths: &Paths) -> TokenStream {
    let opens_under = paths.core("OpensUnder");
    let impls = parents
        .iter()
        .map(|parent| quote_spanned!(parent.span()=> impl #opens_under<#parent> for #ident {}));

    quote!(#(#impls)*)
}

/// A const assertion per parent that it outranks the scope, spanned at the parent.
fn rank_checks(parents: &[syn::Path], ident: &syn::Ident, paths: &Paths) -> TokenStream {
    let scope_trait = paths.core("Scope");
    let checks = parents.iter().map(|parent| {
        let message = format!(
            "a #[scope] parent must outrank the scope: `{}` does not outrank `{}`",
            quote!(#parent).to_string().replace(' ', ""),
            ident.unraw()
        );

        quote_spanned! {parent.span()=>
            const _: () = ::core::assert!(
                <#parent as #scope_trait>::RANK > <#ident as #scope_trait>::RANK,
                #message
            );
        }
    });

    quote!(#(#checks)*)
}

/// Per child, spanned at the child: `impl OpensUnder<S> for Child {}`, which is allowed for a
/// child declared in another crate because `S` is local, and a const assertion that the scope
/// outranks the child.
fn children(children: &[syn::Path], ident: &syn::Ident, paths: &Paths) -> TokenStream {
    let opens_under = paths.core("OpensUnder");
    let scope_trait = paths.core("Scope");
    let items = children.iter().map(|child| {
        let message = format!(
            "a #[scope] child must rank below the scope: `{}` does not rank below `{}`",
            quote!(#child).to_string().replace(' ', ""),
            ident.unraw()
        );

        quote_spanned! {child.span()=>
            impl #opens_under<#ident> for #child {}

            const _: () = ::core::assert!(
                <#ident as #scope_trait>::RANK > <#child as #scope_trait>::RANK,
                #message
            );
        }
    });

    quote!(#(#items)*)
}

/// For a scope with fields: a const assertion that the scope is not zero-sized, so the
/// parameter slot its declaration adds (decided by `size_of`) exists exactly when it has fields,
/// and under `di-check`, `impl Provided<S> for Wiring {}` for that slot. A unit scope gets
/// neither.
fn provided(fields: &Fields, ident: &syn::Ident, paths: &Paths) -> TokenStream {
    if fields.is_empty() {
        return quote!();
    }

    let provide = crate::di::provide_impl(ident, paths);

    quote_spanned! {fields.span()=>
        const _: () = ::core::assert!(
            ::core::mem::size_of::<#ident>() != 0,
            "a #[scope] with fields must not be zero-sized: its value is stored only when it has a \
             size; declare a scope without data as a unit struct"
        );

        #provide
    }
}

#[cfg(test)]
mod tests;
