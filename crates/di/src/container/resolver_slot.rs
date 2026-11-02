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
    /// For a scope under construction whose layout plans no capture. A factory that captures
    /// it anyway gets a pending slot from its construction context, and the built container
    /// gets an attached slot.
    Detached,
}

/// Mutable hydration state shared while a scope is under construction.
#[derive(Default)]
pub(super) struct ScopeResolverState {
    container: Weak<ScopeContainer>,
    deferred: Vec<Arc<dyn crate::primitives::DeferredHydrator>>,
    /// Other pending slots created for the same scope, attached together with this one.
    followers: Vec<ScopeResolverSlot>,
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

    pub(crate) fn detached() -> Self {
        Self {
            state: ScopeResolverSlotState::Detached,
        }
    }

    /// Whether this slot was opened detached, for a scope whose layout plans no capture.
    pub(crate) fn is_detached(&self) -> bool {
        matches!(self.state, ScopeResolverSlotState::Detached)
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

    /// Attaches `container` and hydrates every deferred dependency registered so far,
    /// returning the first hydration error after attempting them all.
    pub(crate) fn attach(&self, container: &Arc<ScopeContainer>) -> crate::Result<()> {
        let ScopeResolverSlotState::Pending(state) = &self.state else {
            return Ok(());
        };
        let (deferred, followers) = {
            let mut state = state.lock().expect("scope resolver slot poisoned");

            state.container = Arc::downgrade(container);

            (
                std::mem::take(&mut state.deferred),
                std::mem::take(&mut state.followers),
            )
        };
        let hydrated = deferred
            .iter()
            .map(|deferred| deferred.hydrate(container))
            .fold(Ok(()), crate::Result::and);

        followers
            .iter()
            .map(|follower| follower.attach(container))
            .fold(hydrated, crate::Result::and)
    }

    /// Attaches `other`, another pending slot created for the same scope, whenever this one
    /// is attached: at once if it already is, otherwise together with it.
    pub(crate) fn absorb(&self, other: ScopeResolverSlot) -> crate::Result<()> {
        let ScopeResolverSlotState::Pending(state) = &self.state else {
            return match self.resolve() {
                Ok(container) => other.attach(&container),
                Err(_) => Ok(()),
            };
        };
        let container = {
            let mut state = state.lock().expect("scope resolver slot poisoned");
            let container = state.container.upgrade();

            if container.is_none() {
                state.followers.push(other.clone());
            }

            container
        };

        match container {
            Some(container) => other.attach(&container),
            None => Ok(()),
        }
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
