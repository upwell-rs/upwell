//! Hot-update preparation against a built candidate root.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use futures::FutureExt;

use super::ScopeContainer;
use crate::descriptors::component::ComponentConstructionContext;
use crate::{ComponentDescriptor, Error, HotUpdateCommit};

impl ScopeContainer {
    /// Prepares the declared hot update of a singleton this candidate root retains.
    ///
    /// The preparation reads the retained instance through [`Previous`](crate::Previous)
    /// and resolves every other parameter from this root, so it observes candidate
    /// dependencies only. It runs under panic containment and never mutates the instance;
    /// the returned commit applies the update when the transition publishes.
    #[doc(hidden)]
    pub async fn prepare_hot_update(
        self: &Arc<Self>,
        descriptor: ComponentDescriptor,
    ) -> crate::Result<HotUpdateCommit> {
        let factory = descriptor
            .hot_update_factory()?
            .ok_or(Error::HotUpdateUnavailable {
                component: descriptor.id,
            })?;
        let previous = self.snapshot_singleton(descriptor)?;
        let mut context = ComponentConstructionContext::new_with_slot(
            descriptor.scope,
            Some(Arc::clone(self)),
            Arc::clone(&self.registry),
            self.inherited_resolvers(),
            self.slot.clone(),
        );

        context.set_previous(Some(previous));

        let prepared = AssertUnwindSafe((factory.construct)(&mut context))
            .catch_unwind()
            .await
            .map_err(|_| Error::HotUpdatePanicked {
                component: descriptor.id,
            })??;

        HotUpdateCommit::take(prepared).ok_or(Error::CandidateFactoryTypeMismatch {
            component: descriptor.id,
            expected: "hot-update commit",
            actual: "<component>",
        })
    }
}
