//! Filling the seed slots of an open scope, for a seed its opening left out.
//!
//! A seed may come from something its protocol holds only for part of the scope's life, such as
//! a request head. The protocol then supplies it at the opening when the opening reads it, and
//! later while it can still produce it, when what it is about to resolve reads it.

use std::any::{Any, TypeId};
use std::sync::Arc;

use upwell_core::{DependencyDescriptor, ScopeKey, TypeDescriptor};

use super::ScopeContainer;
use crate::construct::FromContainer;
use crate::descriptors::BoxedComponent;
use crate::error::Error;
use crate::layout::EagerSelection;

impl ScopeContainer {
    /// Fills this scope's seed slot for `T` from `source`, unless it holds a value. A scope
    /// without a slot for `T` is left alone.
    ///
    /// # Errors
    ///
    /// [`SeedForBuiltComponent`](Error::SeedForBuiltComponent) when a factory builds `T`.
    pub fn ensure_seed<T: Any + Send + Sync>(
        &self,
        source: impl FnOnce() -> T,
    ) -> crate::Result<()> {
        let Some(store) = self.store.slots() else {
            return Ok(());
        };
        let layout = store.layout();
        let Some(slot) = layout.slot_of(TypeId::of::<T>()) else {
            return Ok(());
        };
        let spec = layout.spec(slot);

        if spec.factory.is_some() {
            return Err(Error::SeedForBuiltComponent {
                component: spec.component.name,
                scope: layout.scope().name(),
            });
        }

        if store.is_built(slot) {
            return Ok(());
        }

        store.seed_later(
            slot,
            BoxedComponent::stored(
                TypeDescriptor::of::<T>(std::any::type_name::<T>()),
                source(),
            ),
        )
    }

    /// Extracts `H` as [`extract`](Self::extract) does, first filling this scope's seed slot for
    /// `T` from `source` when resolving `H` reads that seed and the slot is empty. A value
    /// already built, or one that does not read the seed, never calls `source`.
    ///
    /// # Errors
    ///
    /// As [`extract`](Self::extract), and as [`ensure_seed`](Self::ensure_seed).
    pub async fn extract_seeding<H, T>(
        self: &Arc<Self>,
        source: impl FnOnce() -> T,
    ) -> crate::Result<H>
    where
        H: FromContainer,
        T: Any + Send + Sync,
    {
        if let Some(value) = H::read_built(self) {
            return Ok(value);
        }

        if self.reads_empty_seed(TypeId::of::<T>(), &H::dependency()) {
            self.ensure_seed(source)?;
        }

        self.extract_selected::<H>().await
    }

    /// Whether resolving `dependency` from this scope reads its empty seed slot for `seed`:
    /// the slot itself, a slot it reaches over edges of any kind, or a slot an
    /// invocation-context accessor reachable from it reads.
    fn reads_empty_seed(&self, seed: TypeId, dependency: &DependencyDescriptor) -> bool {
        if dependency.config {
            return false;
        }

        let Some(store) = self.store.slots() else {
            return false;
        };
        let layout = store.layout();
        let Some(slot) = layout.slot_of(seed) else {
            return false;
        };

        if store.is_built(slot) {
            return false;
        }

        let can_access = |_: ScopeKey, dependency: ScopeKey| self.can_access(dependency);
        let selector = EagerSelection {
            selection: &self.registry.selection,
            can_access: &can_access,
        };
        layout.read_by(&selector, dependency)[slot.index()]
    }
}
