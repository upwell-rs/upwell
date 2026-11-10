//! The narrow active-root snapshot seam: reboxes one singleton's stored
//! representation into fresh generation-local storage.
//!
//! [`ScopeContainer::snapshot_singleton`] is the only sanctioned way to carry an
//! active singleton into a candidate generation. It reads exactly one concrete
//! component from this scope's local store, runs the descriptor's hidden typed
//! adapter under panic containment, and validates the produced slot — never
//! touching provider aliases, parent scopes, or the resolver path.

use std::panic::{AssertUnwindSafe, catch_unwind};

use upwell_core::{Scope, Singleton};

use super::ScopeContainer;
use crate::descriptors::component::GenerationSnapshot;
use crate::{BoxedComponent, ComponentDescriptor, Error};

impl ScopeContainer {
    /// Snapshots one singleton component out of this container's *local* store into
    /// fresh generation-local storage.
    ///
    /// The descriptor must be singleton-scoped and
    /// [retainable](ComponentDescriptor::is_retainable): a typed descriptor that has not
    /// opted out of retention. Raw [`manual`](ComponentDescriptor::manual) descriptors and
    /// [`without_retention`](ComponentDescriptor::without_retention) descriptors return
    /// [`SnapshotUnavailable`](Error::SnapshotUnavailable).
    /// Only the local concrete slot is consulted — no parent walk, no provider
    /// aliases — and the adapter runs under panic containment, so a panicking
    /// `Clone` surfaces as a redacted error instead of unwinding through the caller.
    /// The produced slot's type is validated against the descriptor before it is
    /// returned; nothing else from this scope (providers, resolvers, parent path) is
    /// copied.
    #[doc(hidden)]
    pub fn snapshot_singleton(
        &self,
        descriptor: ComponentDescriptor,
    ) -> crate::Result<BoxedComponent> {
        if descriptor.scope.id() != Singleton::ID {
            return Err(Error::SnapshotUnavailable {
                component: descriptor.id,
            });
        }

        let active_descriptor = self
            .registry
            .component(descriptor.ty.type_id)
            .ok_or(Error::MissingComponent(descriptor.name))?;
        let Some(snapshot) = active_descriptor
            .generation_snapshot
            .and_then(GenerationSnapshot::retention)
        else {
            return Err(Error::SnapshotUnavailable {
                component: descriptor.id,
            });
        };

        let active = self
            .store
            .component(descriptor.ty.type_id)
            .ok_or(Error::MissingComponent(descriptor.name))?;

        let boxed =
            catch_unwind(AssertUnwindSafe(|| snapshot.snapshot(active))).map_err(|_| {
                Error::SnapshotPanicked {
                    component: descriptor.id,
                }
            })??;

        if boxed.ty.type_id != descriptor.ty.type_id || !snapshot.validates(&boxed) {
            return Err(Error::SnapshotOutputMismatch {
                component: descriptor.id,
            });
        }

        Ok(boxed)
    }
}

#[cfg(test)]
mod tests;
