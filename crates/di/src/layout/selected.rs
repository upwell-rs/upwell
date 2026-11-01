//! The source each single-value trait dependency resolves to from a scope, fixed when the
//! generation is prepared, so reading a built provider never selects one per call.

use std::any::TypeId;
use std::collections::HashMap;

use upwell_core::{ResolutionMode, ScopeKey};

use super::{DepRef, EagerSelection};
use crate::descriptors::ProviderDescriptor;
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

/// The [`SelectedSource`] of every single-value trait dependency of one scope, unqualified
/// and per qualifier.
///
/// A dependency is left out, so it keeps runtime selection, when no provider is selected
/// (none visible, or an ambiguous choice), when the selected provider is transient, or when
/// its instance has no planned slot in the scope or the layouts around it.
#[derive(Debug, Default)]
pub(crate) struct SelectedSources {
    by_trait: HashMap<TypeId, Box<[SelectedSource]>>,
}

impl SelectedSources {
    /// Selects, from the scope `consumer`, the provider of each trait `selector` knows, for
    /// an unqualified dependency and for each qualifier. `stored` places a non-transient
    /// provider's instance in the scope's slots or around them.
    pub(crate) fn plan(
        consumer: ScopeKey,
        selector: &EagerSelection<'_>,
        stored: impl Fn(&ProviderDescriptor) -> Option<DepRef>,
    ) -> Self {
        let mut by_trait = HashMap::new();

        for target in selector.selection.traits() {
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

        Self { by_trait }
    }

    /// The selected source of a dependency on trait `target` with `qualifier`.
    pub(crate) fn get(&self, target: TypeId, qualifier: Option<&str>) -> Option<&SelectedSource> {
        self.by_trait
            .get(&target)?
            .iter()
            .find(|selected| selected.qualifier == qualifier)
    }
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
