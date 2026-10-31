//! The per-scope resolver slot transient construction reads its resolvers from.

use std::sync::{Arc, Weak};

use super::ScopeContainer;
use crate::error::Error;

/// Cloneable attachment point for a scope under construction or already built.
#[derive(Clone)]
pub(crate) struct ScopeResolverSlot {
    pub(super) state: ScopeResolverSlotState,
}

/// Storage used by a pending or attached scope resolver slot.
#[derive(Clone)]
pub(super) enum ScopeResolverSlotState {
    Pending(Arc<std::sync::Mutex<ScopeResolverState>>),
    Attached(Weak<ScopeContainer>),
    /// For a scope under construction whose layout captures nothing, so no handle can
    /// reach it before it is built. The built container gets an attached slot.
    Detached,
}

/// Mutable hydration state shared while a scope is under construction.
#[derive(Default)]
pub(super) struct ScopeResolverState {
    container: Weak<ScopeContainer>,
    deferred: Vec<Arc<dyn crate::primitives::DeferredHydrator>>,
}

impl Default for ScopeResolverSlot {
    fn default() -> Self {
        Self {
            state: ScopeResolverSlotState::Pending(Arc::new(std::sync::Mutex::new(
                ScopeResolverState::default(),
            ))),
        }
    }
}

impl ScopeResolverSlot {
    pub(super) fn attached(container: Weak<ScopeContainer>) -> Self {
        Self {
            state: ScopeResolverSlotState::Attached(container),
        }
    }

    pub(super) fn detached() -> Self {
        Self {
            state: ScopeResolverSlotState::Detached,
        }
    }

    /// The slot the built `container` keeps: this one, or an attached one when this one
    /// was detached.
    pub(super) fn for_container(&self, container: &Weak<ScopeContainer>) -> Self {
        match self.state {
            ScopeResolverSlotState::Detached => Self::attached(container.clone()),
            ScopeResolverSlotState::Pending(_) | ScopeResolverSlotState::Attached(_) => {
                self.clone()
            }
        }
    }

    pub(crate) fn attach(&self, container: &Arc<ScopeContainer>) -> crate::Result<()> {
        let ScopeResolverSlotState::Pending(state) = &self.state else {
            return Ok(());
        };
        let deferred = {
            let mut state = state.lock().expect("scope resolver slot poisoned");

            state.container = Arc::downgrade(container);

            std::mem::take(&mut state.deferred)
        };

        for deferred in deferred {
            deferred.hydrate(container)?;
        }

        Ok(())
    }

    pub(crate) fn resolve(&self) -> crate::Result<Arc<ScopeContainer>> {
        match &self.state {
            ScopeResolverSlotState::Pending(state) => state
                .lock()
                .expect("scope resolver slot poisoned")
                .container
                .upgrade()
                .ok_or(Error::ScopeUnavailable),
            ScopeResolverSlotState::Attached(container) => {
                container.upgrade().ok_or(Error::ScopeUnavailable)
            }
            ScopeResolverSlotState::Detached => Err(Error::ScopeUnavailable),
        }
    }

    pub(crate) fn register_deferred(
        &self,
        deferred: Arc<dyn crate::primitives::DeferredHydrator>,
    ) -> crate::Result<()> {
        let ScopeResolverSlotState::Pending(state) = &self.state else {
            let container = self.resolve()?;

            return deferred.hydrate(&container);
        };
        let container = {
            let mut state = state.lock().expect("scope resolver slot poisoned");

            if let Some(container) = state.container.upgrade() {
                Some(container)
            } else {
                state.deferred.push(Arc::clone(&deferred));

                None
            }
        };

        if let Some(container) = container {
            deferred.hydrate(&container)?;
        }

        Ok(())
    }
}
