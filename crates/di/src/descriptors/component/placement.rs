//! Placements: the scopes one component type builds in.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use upwell_core::{ScopeId, ScopeKey};

use super::{ComponentDescriptor, ComponentFactoryRole};

/// The interned identity of a component placed in one of several scopes.
static PLACEMENT_IDS: OnceLock<Mutex<HashMap<(&'static str, ScopeId), &'static str>>> =
    OnceLock::new();

impl ComponentDescriptor {
    /// Every scope this component builds in, in the order its explicit factories declare
    /// them. A factory without scopes of its own builds in every scope the component
    /// declares, and a component without explicit factories builds in its declared scopes:
    /// `#[component(scope = [A, B])]` places the field-injection default in both.
    ///
    /// Owner rule (2026-11-03): the placements of one type may not share a hierarchy, so a
    /// resolution from any scope chain finds at most one of them. Preparation rejects the
    /// rest with [`Error::OverlappingPlacements`](crate::Error::OverlappingPlacements).
    pub fn placements(&self) -> Vec<ScopeKey> {
        let mut scopes = Vec::new();
        let mut explicit = false;

        if self.declared_scope.is_some() {
            return vec![self.scope];
        }

        for factory in self.factories_with(ComponentFactoryRole::Explicit) {
            explicit = true;
            push_new(&mut scopes, self.factory_scopes(factory));
        }

        if !explicit {
            push_new(&mut scopes, self.declared_scopes());
        }

        scopes
    }

    /// Splits this component into one descriptor per placement.
    ///
    /// A component with one placement in its declared scope is returned unchanged. Every
    /// other descriptor pins its scope and records the declared one, so expanding a placement
    /// again returns it as it is. When there are several placements, each gets its own stable
    /// identity (`<id>@<scope id>`), because the effective graph, hook owners and tooling
    /// all key components by id.
    pub(crate) fn expand_placements(&self) -> Vec<ComponentDescriptor> {
        let placements = self.placements();
        let several = placements.len() > 1;

        if self.declared_scope.is_some() || placements == [self.scope] {
            return vec![*self];
        }

        placements
            .into_iter()
            .map(|scope| Self {
                id: if several {
                    placement_id(self.id, scope.id())
                } else {
                    self.id
                },
                scope,
                declared_scope: Some(self.scope),
                ..*self
            })
            .collect()
    }
}

/// Appends the scopes of `from` that `scopes` does not hold yet, keeping their order.
fn push_new(scopes: &mut Vec<ScopeKey>, from: &[ScopeKey]) {
    for scope in from {
        if !scopes.contains(scope) {
            scopes.push(*scope);
        }
    }
}

/// The identity of the placement of component `id` in `scope`, allocated once per pair so
/// repeated preparation and reloads never grow it.
fn placement_id(id: &'static str, scope: ScopeId) -> &'static str {
    let mut interned = PLACEMENT_IDS
        .get_or_init(Mutex::default)
        .lock()
        .expect("the placement id table lock is poisoned, so a holder panicked: this is a bug");

    interned
        .entry((id, scope))
        .or_insert_with(|| Box::leak(format!("{id}@{scope}").into_boxed_str()))
}
