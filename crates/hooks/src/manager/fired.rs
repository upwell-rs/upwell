use upwell_core::TypeDescriptor;

use crate::{HookKind, Result};

/// One hook outcome yielded by [`HookManager::fire_streamed`](super::HookManager::fire_streamed)
/// as soon as the hook completes.
#[must_use = "observer and participant failures are reported only through the fired outcomes"]
#[non_exhaustive]
pub enum FiredHook<K: HookKind> {
    /// A participant ran. Participants are yielded sequentially in registration order.
    Participant {
        /// The component the hook is declared on.
        component: TypeDescriptor,
        /// The participant's outcome.
        outcome: Result<K::Output>,
    },
    /// An observer ran. Observers run concurrently once every participant accepted and
    /// are yielded in completion order.
    Observer {
        /// The component the hook is declared on.
        component: TypeDescriptor,
        /// Whether the observer completed. Never interpreted by the runtime.
        outcome: Result<()>,
    },
}

impl<K: HookKind> FiredHook<K> {
    /// The component the hook is declared on.
    pub fn component(&self) -> TypeDescriptor {
        match self {
            Self::Participant { component, .. } | Self::Observer { component, .. } => *component,
        }
    }

    /// Whether this outcome ended the event, per [`HookKind::ends_event`]. Only a
    /// participant can end an event; it is then the last item the stream yields.
    pub fn ends_event(&self) -> bool {
        match self {
            Self::Participant { outcome, .. } => K::ends_event(outcome),
            Self::Observer { .. } => false,
        }
    }
}

/// What one [`HookManager::fire`](super::HookManager::fire) ran: every participant outcome
/// in registration order, every observer outcome in registration order, and whether a
/// participant ended the event.
#[must_use = "observer and participant failures are reported only through the fired outcomes"]
#[non_exhaustive]
pub struct FiredHooks<K: HookKind> {
    /// Each participant that ran and its outcome. The last one ended the event when
    /// [`ended`](Self::ended) is set.
    pub participants: Vec<(TypeDescriptor, Result<K::Output>)>,
    /// Each observer that ran and whether it completed. Empty when the event ended.
    pub observers: Vec<(TypeDescriptor, Result<()>)>,
    /// Whether a participant's outcome ended the event, per [`HookKind::ends_event`].
    pub ended: bool,
}

impl<K: HookKind> FiredHooks<K> {
    /// Collects registration-indexed outcomes, restoring registration order for observers
    /// that completed out of order.
    pub(super) fn collect(mut fired: Vec<(usize, FiredHook<K>)>) -> Self {
        let mut collected = Self {
            participants: Vec::new(),
            observers: Vec::new(),
            ended: false,
        };

        fired.sort_by_key(|(index, _)| *index);

        for (_, hook) in fired {
            collected.ended |= hook.ends_event();

            match hook {
                FiredHook::Participant { component, outcome } => {
                    collected.participants.push((component, outcome));
                }
                FiredHook::Observer { component, outcome } => {
                    collected.observers.push((component, outcome));
                }
            }
        }

        collected
    }
}
