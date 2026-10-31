//! The live-slot rebinding seam: finds the exact [`LiveCell`] a root stores for one
//! singleton dependency target, so a runtime transition can republish the cells its
//! retained consumers hold.

use upwell_core::{ProviderMappingId, Scope, Singleton};

use super::ScopeContainer;
use crate::{EffectiveTarget, Error, LiveCell};

impl ScopeContainer {
    /// Returns the live cell this container's *local* store holds for `target`.
    ///
    /// A singleton component target yields its concrete slot; a singleton provider
    /// target yields the erased trait slot registered for that exact mapping, which is
    /// the cell a `Dep<dyn Trait>` consumer resolved. Other targets, missing slots, and
    /// handles that store no live cell are reported as
    /// [`LiveCellUnavailable`](Error::LiveCellUnavailable).
    #[doc(hidden)]
    pub fn live_cell(&self, target: EffectiveTarget) -> crate::Result<LiveCell> {
        let component = target.component_id().unwrap_or("<unknown>");
        let unavailable = Error::LiveCellUnavailable { component };

        match target {
            EffectiveTarget::Component { scope, .. } | EffectiveTarget::Provider { scope, .. }
                if scope != Singleton::ID =>
            {
                Err(unavailable)
            }
            EffectiveTarget::Component { component, .. } => {
                self.component_live_cell(component).ok_or(unavailable)
            }
            EffectiveTarget::Provider { mapping, .. } => {
                self.provider_live_cell(mapping).ok_or(unavailable)
            }
            EffectiveTarget::Config { .. } | EffectiveTarget::Dynamic { .. } => Err(unavailable),
        }
    }

    fn component_live_cell(&self, component: &str) -> Option<LiveCell> {
        let descriptor = self.registry.component_by_id(component)?;
        let stored = self.store.component(descriptor.ty.type_id)?;

        descriptor.generation_snapshot?.live_cell(stored)
    }

    fn provider_live_cell(&self, mapping: ProviderMappingId) -> Option<LiveCell> {
        let descriptor = self.registry.component_by_id(mapping.component)?;
        let provider = self
            .registry
            .providers_for(descriptor.ty.type_id)
            .iter()
            .find(|provider| provider.mapping_id(&descriptor) == mapping)?;
        let value = self.store.provider_value(provider)?;

        (provider.live)(value)
    }
}

#[cfg(test)]
mod tests;
