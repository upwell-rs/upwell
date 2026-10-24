//! Final-release tracking for displaced runtime generations.
//!
//! A committed transition displaces its base generation, but pinned [`RuntimeView`]s, open
//! scopes, and other owners of the displaced root keep it alive afterwards. Each committed
//! generation's root retains a [`ReleaseGuard`] that drops after the root's own instances.
//! Once the generation has been displaced, the guard reports the root's final release as one
//! `displaced runtime generation released` event on `upwell::transition`. Until then,
//! the generation is listed by [`AppRuntime::displaced_generations`]. A generation
//! that is still current when its root drops, as at shutdown, reports nothing.
//!
//! [`RuntimeView`]: super::RuntimeView
//! [`AppRuntime::displaced_generations`]: crate::AppRuntime::displaced_generations

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use upwell_core::RuntimeGenerationId;

use super::TransitionAttemptId;
use crate::runtime::TRANSITION_TARGET;

/// A runtime generation that a committed transition displaced and whose root is still alive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct DisplacedGeneration {
    /// The displaced generation.
    pub generation: RuntimeGenerationId,
    /// The attempt whose commit displaced it.
    pub attempt: TransitionAttemptId,
    /// When that commit published the successor generation.
    pub displaced_at: Instant,
}

/// The displaced generations of one runtime whose roots have not been released yet.
#[derive(Default)]
pub(crate) struct DisplacedGenerations {
    alive: Mutex<BTreeMap<RuntimeGenerationId, DisplacedGeneration>>,
}

impl DisplacedGenerations {
    /// Records that `attempt` displaced `generation`. The caller must still pin the
    /// generation, so its root cannot be released before it is recorded.
    pub(super) fn displace(&self, generation: RuntimeGenerationId, attempt: TransitionAttemptId) {
        let displaced = DisplacedGeneration {
            generation,
            attempt,
            displaced_at: Instant::now(),
        };

        self.alive().insert(generation, displaced);
    }

    /// The displaced generations whose roots are still alive, oldest first.
    pub(crate) fn alive_generations(&self) -> Vec<DisplacedGeneration> {
        self.alive().values().copied().collect()
    }

    fn release(&self, generation: RuntimeGenerationId) -> Option<DisplacedGeneration> {
        self.alive().remove(&generation)
    }

    /// Tolerates poisoning: every critical section is a single map operation, so a
    /// panicking holder cannot leave the map half-updated.
    fn alive(&self) -> MutexGuard<'_, BTreeMap<RuntimeGenerationId, DisplacedGeneration>> {
        self.alive.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Retained by a committed generation's root; reports the root's final release once a
/// later commit displaced the generation.
pub(super) struct ReleaseGuard {
    generation: RuntimeGenerationId,
    displaced: Arc<DisplacedGenerations>,
}

impl ReleaseGuard {
    pub(super) fn new(
        generation: RuntimeGenerationId,
        displaced: Arc<DisplacedGenerations>,
    ) -> Self {
        Self {
            generation,
            displaced,
        }
    }
}

impl Drop for ReleaseGuard {
    /// Runs on whichever thread releases the root's last owner, so it only takes one short
    /// lock and emits one event, and never panics.
    fn drop(&mut self) {
        let Some(displaced) = self.displaced.release(self.generation) else {
            return;
        };
        let released_after_us =
            u64::try_from(displaced.displaced_at.elapsed().as_micros()).unwrap_or(u64::MAX);

        tracing::info!(
            target: TRANSITION_TARGET,
            generation = displaced.generation.get(),
            attempt = displaced.attempt.get(),
            released_after_us,
            "displaced runtime generation released"
        );
    }
}
