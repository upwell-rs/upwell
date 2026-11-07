//! The slots of a scope an entry reaches through invocation-context edges.
//!
//! A `Current<T>` or `Scoped<S, T>` edge is resolved from the active invocation when it is
//! read, never with its consumer, and its consumer is often a singleton built long before the
//! entry runs. An entry that reaches such an edge, through its own targets or through any
//! component they depend on in any scope, therefore declares the slots of its scope the edge
//! resolves to: its scope opens for them, the seeds they read are captured, and building them
//! on demand is not undeclared. Nothing they reach is built up front.
//!
//! The edge of a `Scoped<S, T>` does not carry `S`, so it is planned in the scope that
//! provides `T`, as `Current<T>` is.

use std::any::TypeId;
use std::collections::HashSet;

use upwell_core::{DependencyDescriptor, ResolutionMode};

use super::SlotLayout;
use super::entry_plan::EagerSelection;
use crate::descriptors::ComponentDescriptor;
use crate::registry::ProviderSelectionModel;

impl SlotLayout {
    /// The slots of this scope the invocation-context edges reachable from `targets` resolve
    /// to, with every slot of this scope they depend on.
    pub(super) fn accessed(
        &self,
        selector: &EagerSelection<'_>,
        targets: &[DependencyDescriptor],
    ) -> Box<[bool]> {
        let mut accessed = vec![false; self.slots.len()].into_boxed_slice();
        let mut visited = HashSet::new();
        let mut pending: Vec<TypeId> = targets
            .iter()
            .filter(|target| !target.config)
            .map(|target| target.ty.type_id)
            .collect();

        while let Some(target) = pending.pop() {
            if !visited.insert(target) {
                continue;
            }

            for dependency in dependencies_of(selector.selection, target) {
                if dependency.config {
                    continue;
                }

                if dependency.resolution == ResolutionMode::Current {
                    self.for_each_selected_slot(
                        selector,
                        dependency.ty.type_id,
                        dependency.qualifier,
                        dependency.cardinality,
                        |slot| self.include(selector, slot, &mut accessed),
                    );
                }

                pending.push(dependency.ty.type_id);
            }
        }

        accessed
    }
}

/// The dependency edges of every factory that can build `target`: each placement of the
/// concrete type, or of every concrete type providing the trait it names.
fn dependencies_of(
    selection: &ProviderSelectionModel,
    target: TypeId,
) -> impl Iterator<Item = DependencyDescriptor> + '_ {
    let placements = selection.placements(target);
    let concrete = placements.iter().copied().chain(
        selection
            .providers_for_trait(target)
            .iter()
            .flat_map(|provider| selection.placements(provider.concrete_ty.type_id))
            .copied(),
    );

    concrete.flat_map(factory_dependencies)
}

/// The dependency edges of `component`'s effective factory, none for a seed.
fn factory_dependencies(component: ComponentDescriptor) -> Vec<DependencyDescriptor> {
    match component.effective_factory() {
        Ok(Some(factory)) => (factory.dependencies)(),
        Ok(None) | Err(_) => Vec::new(),
    }
}
