//! Pure planning work split across scoped threads once it is large enough to pay for them.

/// The amount of planning work, in unique entry plans or components, from which planning
/// runs on more than one thread. Measured with the `entry_planning` bench: threads save 6-10%
/// from about 5 000 components, nothing at 2 000, and cost 15% at 1 000 plans on a small graph.
const PARALLEL_THRESHOLD: usize = 2_048;

/// How generation planning splits independent, pure work across threads.
///
/// Work below the threshold, on wasm, or with one thread available runs on the calling
/// thread. Either way the results come back in input order, so a plan never depends on the
/// number of threads it was made with.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Parallelism {
    threshold: usize,
    threads: usize,
}

impl Parallelism {
    /// Splits work of at least `threshold` units across up to `threads` threads.
    pub(crate) const fn new(threshold: usize, threads: usize) -> Self {
        Self { threshold, threads }
    }

    /// The default threshold over every thread the platform reports available.
    pub(crate) fn available() -> Self {
        Self::new(PARALLEL_THRESHOLD, available_threads())
    }

    /// Maps `items` through `map`, in order. `work` measures how much planning the items
    /// hold, against the threshold.
    pub(crate) fn map<T, R>(&self, items: &[T], work: usize, map: impl Fn(&T) -> R + Sync) -> Vec<R>
    where
        T: Sync,
        R: Send,
    {
        let threads = self.threads.min(items.len());

        if threads <= 1 || work < self.threshold {
            return items.iter().map(map).collect();
        }

        let chunk = items.len().div_ceil(threads);

        std::thread::scope(|scope| {
            let workers: Vec<_> = items
                .chunks(chunk)
                .map(|chunk| scope.spawn(|| chunk.iter().map(&map).collect::<Vec<_>>()))
                .collect();

            workers
                .into_iter()
                .flat_map(|worker| {
                    worker
                        .join()
                        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
                })
                .collect()
        })
    }
}

#[cfg(not(target_family = "wasm"))]
fn available_threads() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
}

#[cfg(target_family = "wasm")]
fn available_threads() -> usize {
    1
}
