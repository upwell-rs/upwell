//! The sources each trait dependency resolves to from a scope, fixed when the generation is
//! prepared, so reading built providers never selects them per call.

use std::any::TypeId;
use std::collections::HashMap;

use upwell_core::{Cardinality, ResolutionMode, ScopeKey};

use super::{DepRef, EagerSelection, ProvidedRef};
use crate::descriptors::ProviderDescriptor;
use crate::registry::selection::ProviderSelection;
use crate::registry::selection::provider_matches;

/// The provider runtime selection picks for one trait, unqualified or by `qualifier`, from a
/// scope, and where its instance sits.
///
/// The scope's layout plans with the scopes reachable on *every* chain it may open in, while
/// runtime selection sees the scopes on the chain it actually opened in. Those can be more:
/// a scope opened under a parent it may also open without sees that parent's providers.
/// `shadowed_by` lists each such scope whose providers selection would try before the
/// selected one, so a container whose chain holds one of them selects at runtime instead.
/// Scopes the layout reaches but the chain lacks need no list: their providers were passed
/// over by the plan, unless they hold the selected one, which the chain then cannot read.
#[derive(Debug)]
pub(crate) struct SelectedSource {
    pub(crate) qualifier: Option<&'static str>,
    pub(crate) source: DepRef,
    pub(crate) shadowed_by: Box<[ScopeKey]>,
}

/// Every provider runtime selection gathers for a collection or keyed dependency on one trait
/// from a scope, in the order it gathers them, and where each instance sits. A keyed map holds
/// the provider that wins each qualifier.
///
/// `shadowed_by` lists the scopes the layout cannot reach on every chain that hold a provider
/// of the trait, as [`SelectedSource::shadowed_by`] does for one value: a container whose chain
/// holds one gathers more providers, or other winners, and selects at runtime instead.
#[derive(Debug)]
pub(crate) struct SelectedMany {
    pub(crate) providers: Box<[ProvidedRef]>,
    pub(crate) shadowed_by: Box<[ScopeKey]>,
}

/// The [`SelectedSource`] of every single-value trait dependency of one scope, unqualified
/// and per qualifier, and the [`SelectedMany`] of every collection and keyed one.
///
/// A dependency is left out, so it keeps runtime selection, when no provider is selected
/// (none visible, or an ambiguous choice), when a selected provider is transient, or when
/// an instance has no planned slot in the scope or the layouts around it.
#[derive(Debug, Default)]
pub(crate) struct SelectedSources {
    by_trait: HashMap<TypeId, Box<[SelectedSource]>>,
    collections: HashMap<TypeId, SelectedMany>,
    keyed: HashMap<TypeId, SelectedMany>,
}

impl SelectedSources {
    /// Selects, from the scope `consumer`, the provider of each trait `selector` knows, for
    /// an unqualified dependency and for each qualifier, and the providers a collection and a
    /// keyed dependency on it gather. `stored` places a non-transient provider's instance in
    /// the scope's slots or around them.
    pub(crate) fn plan(
        consumer: ScopeKey,
        selector: &EagerSelection<'_>,
        stored: impl Fn(&ProviderDescriptor) -> Option<DepRef>,
    ) -> Self {
        let mut by_trait = HashMap::new();
        let mut collections = HashMap::new();
        let mut keyed = HashMap::new();

        for target in selector.selection.traits() {
            let many = |cardinality| select_many(consumer, selector, target, cardinality, &stored);

            if let Some(selected) = many(Cardinality::Collection) {
                collections.insert(target, selected);
            }

            if let Some(selected) = many(Cardinality::Keyed) {
                keyed.insert(target, selected);
            }

            let mut sources = Vec::new();
            let qualifiers = std::iter::once(None).chain(
                selector
                    .selection
                    .providers_for_trait(target)
                    .iter()
                    .map(|provider| Some(provider.qualifier)),
            );

            for qualifier in qualifiers {
                if sources
                    .iter()
                    .any(|planned: &SelectedSource| planned.qualifier == qualifier)
                {
                    continue;
                }

                sources.extend(select(consumer, selector, target, qualifier, &stored));
            }

            if !sources.is_empty() {
                by_trait.insert(target, sources.into_boxed_slice());
            }
        }

        Self {
            by_trait,
            collections,
            keyed,
        }
    }

    /// The selected source of a dependency on trait `target` with `qualifier`.
    pub(crate) fn get(&self, target: TypeId, qualifier: Option<&str>) -> Option<&SelectedSource> {
        self.by_trait
            .get(&target)?
            .iter()
            .find(|selected| selected.qualifier == qualifier)
    }

    /// The selected providers of a collection or keyed dependency on trait `target`. `None` for
    /// a single value.
    pub(crate) fn many(&self, target: TypeId, cardinality: Cardinality) -> Option<&SelectedMany> {
        match cardinality {
            Cardinality::Collection => self.collections.get(&target),
            Cardinality::Keyed => self.keyed.get(&target),
            Cardinality::One => None,
        }
    }
}

/// The providers runtime selection gathers for a `cardinality` dependency on `target` from
/// `consumer`, when every one of them is a planned, non-transient instance.
fn select_many(
    consumer: ScopeKey,
    selector: &EagerSelection<'_>,
    target: TypeId,
    cardinality: Cardinality,
    stored: &impl Fn(&ProviderDescriptor) -> Option<DepRef>,
) -> Option<SelectedMany> {
    let selection = selector.selection;
    let gathered: Vec<ProviderSelection> = match cardinality {
        Cardinality::Collection => selection.select_runtime_collection(
            target,
            ResolutionMode::Eager,
            consumer,
            &selector.can_access,
        ),
        Cardinality::Keyed => selection.select_runtime_keyed(
            target,
            ResolutionMode::Eager,
            consumer,
            &selector.can_access,
        ),
        Cardinality::One => return None,
    };
    let mut providers = Vec::with_capacity(gathered.len());

    for chosen in gathered {
        let provider = chosen.provider;

        if selection
            .component(provider.concrete_ty.type_id)?
            .scope
            .is_transient()
        {
            return None;
        }

        providers.push(ProvidedRef {
            qualifier: provider.qualifier,
            source: stored(&provider)?,
        });
    }

    Some(SelectedMany {
        providers: providers.into_boxed_slice(),
        shadowed_by: unreached(consumer, selector, target),
    })
}

/// The scopes `consumer` cannot reach on every chain that hold a non-transient provider of
/// `target`.
fn unreached(consumer: ScopeKey, selector: &EagerSelection<'_>, target: TypeId) -> Box<[ScopeKey]> {
    let mut unreached: Vec<ScopeKey> = Vec::new();

    for provider in selector.selection.providers_for_trait(target) {
        let Some(component) = selector.selection.component(provider.concrete_ty.type_id) else {
            continue;
        };
        let scope = component.scope;

        if scope.is_transient()
            || (selector.can_access)(consumer, scope)
            || unreached.contains(&scope)
        {
            continue;
        }

        unreached.push(scope);
    }

    unreached.into_boxed_slice()
}

/// The source of the provider runtime selection picks for `target` with `qualifier` from
/// `consumer`, when it is a planned, non-transient instance.
fn select(
    consumer: ScopeKey,
    selector: &EagerSelection<'_>,
    target: TypeId,
    qualifier: Option<&'static str>,
    stored: &impl Fn(&ProviderDescriptor) -> Option<DepRef>,
) -> Option<SelectedSource> {
    let chosen = selector
        .selection
        .select_runtime_one(
            target,
            qualifier,
            ResolutionMode::Eager,
            consumer,
            &selector.can_access,
        )?
        .provider;
    let owner = selector
        .selection
        .component(chosen.concrete_ty.type_id)?
        .scope;

    if owner.is_transient() {
        return None;
    }

    Some(SelectedSource {
        qualifier,
        source: stored(&chosen)?,
        shadowed_by: shadowing(consumer, selector, target, qualifier, owner),
    })
}

/// The scopes `consumer` cannot reach on every chain whose providers of `target` matching
/// `qualifier` selection tries before those of `owner`, the scope of the selected one.
///
/// Selection tries scopes by ascending rank, then id, and transient providers do not take
/// part in that order.
fn shadowing(
    consumer: ScopeKey,
    selector: &EagerSelection<'_>,
    target: TypeId,
    qualifier: Option<&str>,
    owner: ScopeKey,
) -> Box<[ScopeKey]> {
    let selected = (owner.rank(), owner.id());
    let mut shadowing: Vec<ScopeKey> = Vec::new();

    for provider in selector.selection.providers_for_trait(target) {
        let Some(component) = selector.selection.component(provider.concrete_ty.type_id) else {
            continue;
        };
        let scope = component.scope;

        if !provider_matches(provider, qualifier)
            || scope.is_transient()
            || (scope.rank(), scope.id()) >= selected
            || (selector.can_access)(consumer, scope)
            || shadowing.contains(&scope)
        {
            continue;
        }

        shadowing.push(scope);
    }

    shadowing.into_boxed_slice()
}
