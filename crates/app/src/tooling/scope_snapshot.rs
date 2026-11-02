//! Callback-free construction facts for each protocol-owned scope: the eager set every
//! opening builds and the components no entry reaches.

use std::any::TypeId;
use std::collections::{HashMap, HashSet};

use upwell_core::ScopeId;
use upwell_di::{DependencyTarget, SlotLayout};

use super::snapshot::{ComponentSnapshot, DependencySnapshot, EntrySnapshot};
use crate::scope::ScopeLayouts;

/// Construction facts for the prepared slot layout of one scope.
#[derive(Clone)]
pub(crate) struct ScopeLayoutSnapshot {
    /// The IDs of the eager components every opening builds, in construction order.
    pub(crate) eager: Vec<&'static str>,
    /// The IDs of the scope's seeds, which whoever opens the scope fills, in layout order.
    pub(crate) seeds: Vec<&'static str>,
    /// The IDs of the factory-backed components nothing reaches from an entry, directly or
    /// through other components of any scope, eager or lazy, in construction order. `None`
    /// when no entry opens the scope, so every component waits for an on-demand resolution.
    pub(crate) unreached: Option<Vec<&'static str>>,
}

impl ScopeLayoutSnapshot {
    /// Captures every scope layout of `layouts` from the selected edges of `components` and
    /// `entries`.
    pub(crate) fn capture_all(
        components: &[ComponentSnapshot],
        entries: &[EntrySnapshot],
        layouts: &ScopeLayouts,
    ) -> HashMap<ScopeId, Self> {
        let reached = Reach::new(components).walk_entries(entries);
        let entered: HashSet<_> = entries
            .iter()
            .map(|entry| entry.descriptor.scope.id())
            .collect();

        layouts
            .iter()
            .map(|(scope, layout)| {
                let unreached = entered
                    .contains(scope)
                    .then(|| unreached_in(layout, &reached));
                let eager = layout
                    .eager_components()
                    .map(|component| component.id)
                    .collect();

                let seeds = layout.seeds().map(|seed| seed.id).collect();

                (
                    *scope,
                    Self {
                        eager,
                        seeds,
                        unreached,
                    },
                )
            })
            .collect()
    }
}

/// The factory-backed components of `layout` outside `reached`.
fn unreached_in(layout: &SlotLayout, reached: &HashSet<TypeId>) -> Vec<&'static str> {
    layout
        .components()
        .filter(|component| component.effective_factory().ok().flatten().is_some())
        .filter(|component| !reached.contains(&component.ty.type_id))
        .map(|component| component.id)
        .collect()
}

/// A walk over the selected dependency edges of the prepared components.
struct Reach<'a> {
    components: HashMap<TypeId, &'a ComponentSnapshot>,
    reached: HashSet<TypeId>,
}

impl<'a> Reach<'a> {
    fn new(components: &'a [ComponentSnapshot]) -> Self {
        Self {
            components: components
                .iter()
                .map(|component| (component.descriptor.ty.type_id, component))
                .collect(),
            reached: HashSet::new(),
        }
    }

    /// The components reachable from `entries` and from the eager components, which every
    /// entry of their scope builds.
    fn walk_entries(mut self, entries: &[EntrySnapshot]) -> HashSet<TypeId> {
        let eager: Vec<_> = self
            .components
            .iter()
            .filter(|(_, component)| component.descriptor.construction.is_eager())
            .map(|(type_id, _)| *type_id)
            .collect();

        for entry in entries {
            self.follow(&entry.targets);
        }

        for type_id in eager {
            self.visit(type_id);
        }

        self.reached
    }

    fn follow(&mut self, dependencies: &[DependencySnapshot]) {
        for selected in dependencies.iter().flat_map(|edge| &edge.selected) {
            self.visit(match selected.target {
                DependencyTarget::Component(component) => component.ty.type_id,
                DependencyTarget::Provider(provider) => provider.concrete_ty.type_id,
            });
        }
    }

    fn visit(&mut self, type_id: TypeId) {
        if !self.reached.insert(type_id) {
            return;
        }

        let Some(&component) = self.components.get(&type_id) else {
            return;
        };

        self.follow(&component.dependencies);

        for hook in &component.hooks {
            self.follow(&hook.dependencies);
        }
    }
}
