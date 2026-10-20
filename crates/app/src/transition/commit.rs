//! The infallible actions a terminal publication applies once the candidate generation
//! is current.

use std::panic::{AssertUnwindSafe, catch_unwind};

use upwell_di::{HotUpdateCommit, LiveRebind};

use crate::runtime::TRANSITION_TARGET;

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
    ///
    /// A hot update's apply action is user code that must not panic, but it runs after
    /// the candidate generation is already published. A panic is therefore contained and
    /// traced: the remaining updates still apply and the publication completes, with the
    /// panicking component left in whatever state its action reached.
    pub(crate) fn commit(self) {
        for rebind in self.rebinds {
            rebind.commit();
        }

        for update in self.hot_updates {
            let component = update.component();

            if catch_unwind(AssertUnwindSafe(|| update.commit())).is_err() {
                tracing::error!(
                    target: TRANSITION_TARGET,
                    component,
                    "hot update panicked while applying after publication"
                );
            }
        }
    }
}
