//! The infallible actions a terminal publication applies once the candidate generation
//! is current.

use upwell_di::{HotUpdateCommit, LiveRebind};

/// Prepared live rebinds and hot updates for one transition.
///
/// Every action was prepared before commit. [`commit`](Self::commit) applies the live
/// rebinds first, so a hot update observes its component's rebound live dependencies,
/// then the hot updates. Dropping the value without committing applies nothing.
#[derive(Default)]
pub(crate) struct CommitActions {
    rebinds: Vec<LiveRebind>,
    hot_updates: Vec<HotUpdateCommit>,
}

impl CommitActions {
    pub(crate) fn new(rebinds: Vec<LiveRebind>, hot_updates: Vec<HotUpdateCommit>) -> Self {
        Self {
            rebinds,
            hot_updates,
        }
    }

    /// Applies every prepared live rebind, then every prepared hot update.
    pub(crate) fn commit(self) {
        for rebind in self.rebinds {
            rebind.commit();
        }

        for update in self.hot_updates {
            update.commit();
        }
    }
}
