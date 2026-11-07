//! The container kind a scope's instances live in, fixed by its scope type's generation policy.

use upwell_core::{ScopeGenerations, ScopeKey};

use crate::descriptors::component::HandleStorage;

/// How an open instance of a scope stores its slots, chosen from the scope type's
/// [`ScopeGenerations`] when its layout is planned, never from its identity.
///
/// Both kinds keep the generation the instance opened under, so both store fixed handles.
/// A rebinding kind, rebuilt onto each new generation with swappable handles, would be a
/// third variant once the policy exists.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContainerKind {
    /// A call, request or message: slots are plain values set while the scope opens, and a
    /// slot the opening left out is built into a second set allocated by the first such build.
    Pinned,
    /// A connection: every slot is a once-cell allocated when the scope opens and ensured on
    /// first use, concurrently with other slots. A failed build is not kept, so the next
    /// resolution builds the slot again.
    Durable,
}

impl ContainerKind {
    /// The kind `scope`'s generation policy selects.
    ///
    /// A policy this version does not know keeps its generation, so it gets a pinned container.
    pub(crate) fn of(scope: ScopeKey) -> Self {
        match scope.generations() {
            ScopeGenerations::Durable => Self::Durable,
            _ => Self::Pinned,
        }
    }

    /// How the factories building a container of this kind store their handles.
    pub(crate) fn handle_storage(self) -> HandleStorage {
        match self {
            Self::Pinned | Self::Durable => HandleStorage::Fixed,
        }
    }
}
