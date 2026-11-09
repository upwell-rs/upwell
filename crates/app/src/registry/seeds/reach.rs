//! Which entries reach each component, so a requirement is checked only against the openers
//! that can open an instance for it.

use std::any::TypeId;
use std::collections::{HashMap, HashSet};

use upwell_core::EntryKey;
use upwell_di::{ComponentDescriptor, ProviderDescriptor};

use crate::entry::EntryDescriptor;

/// The entries that reach each component placement, by component id, over every dependency
/// edge and trait provider.
pub(super) struct EntryReach<'a>(HashMap<&'static str, HashSet<&'a EntryKey>>);

impl<'a> EntryReach<'a> {
    /// Walks from every entry's targets through the dependencies of the components they reach.
    pub(super) fn of(
        components: &[ComponentDescriptor],
        providers: &[ProviderDescriptor],
        entries: &'a [EntryDescriptor],
    ) -> Self {
        let mut by_type: HashMap<TypeId, Vec<&ComponentDescriptor>> = HashMap::new();
        let mut reach: HashMap<&'static str, HashSet<&'a EntryKey>> = HashMap::new();

        for component in components {
            by_type
                .entry(component.ty.type_id)
                .or_default()
                .push(component);
        }

        for entry in entries {
            let mut pending: Vec<TypeId> = (entry.targets)()
                .into_iter()
                .map(|target| target.ty.type_id)
                .collect();
            let mut seen: HashSet<TypeId> = HashSet::new();

            while let Some(ty) = pending.pop() {
                if !seen.insert(ty) {
                    continue;
                }

                let provided = providers
                    .iter()
                    .filter(|provider| provider.trait_ty.type_id == ty)
                    .map(|provider| provider.concrete_ty.type_id);

                pending.extend(provided);

                for component in by_type.get(&ty).into_iter().flatten() {
                    reach.entry(component.id).or_default().insert(&entry.key);
                    pending.extend(
                        component
                            .dependencies()
                            .into_iter()
                            .map(|dependency| dependency.ty.type_id),
                    );
                }
            }
        }

        Self(reach)
    }

    /// The entries that reach `component`, or `None` when every opener of its scope must be
    /// assumed to reach it: an eager component is built on every opening, and one no entry
    /// reaches may still be resolved by any instance.
    pub(super) fn of_component(
        &self,
        component: &ComponentDescriptor,
    ) -> Option<&HashSet<&'a EntryKey>> {
        if component.construction.is_eager() {
            return None;
        }

        self.0.get(component.id)
    }
}
