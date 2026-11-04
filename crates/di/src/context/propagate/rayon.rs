//! rayon integration: pool work that carries a captured context.
//!
//! rayon has no per-job hook, so every closure handed to the pool is bound to the capture
//! ([`CapturedContext::bind`]). A bound job that blocks in a nested `join`, `scope` or
//! parallel iterator may run other jobs of the pool on its thread while it waits; an unbound
//! job run there sees the waiting job's context. Bind every job of a pool shared with
//! context-carrying work, or give that work its own pool.

use super::CapturedContext;

impl CapturedContext {
    /// Runs `body` in `pool` inside the captured context.
    pub fn install<R: Send>(self, pool: &rayon::ThreadPool, body: impl FnOnce() -> R + Send) -> R {
        pool.install(self.bind(body))
    }

    /// `rayon::join`, running both closures inside the captured context, whichever thread
    /// steals them.
    pub fn join<A, B, RA, RB>(self, left: A, right: B) -> (RA, RB)
    where
        A: FnOnce() -> RA + Send,
        B: FnOnce() -> RB + Send,
        RA: Send,
        RB: Send,
    {
        let other = self.clone();

        rayon::join(self.bind(left), other.bind(right))
    }

    /// `rayon::spawn`, running `body` on the global pool inside the captured context.
    pub fn spawn_rayon(self, body: impl FnOnce() + Send + 'static) {
        rayon::spawn(self.bind(body));
    }
}
