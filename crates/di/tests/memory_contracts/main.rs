//! Deterministic memory contracts for the DI container hot paths.
//!
//! These are complexity/leak contracts, not wall-clock benchmarks: they assert allocation counts
//! and live-byte deltas, which are independent of runner speed and so run on every pull request
//! (the statistical memory *trend* across graph sizes lives in `benchmarks/di_graph_memory`). The
//! properties proven here:
//!
//! - resolving a component (`get`) allocates nothing — it is an `Arc` bump and a map walk;
//! - request-time extraction (`extract`) of a built component allocates nothing, so it leaks
//!   nothing either;
//! - extracting a built trait target from a scope opened from its layout allocates nothing,
//!   whether it reads the layout's selected source or an entry plan's pre-resolved parameter,
//!   and a collection or keyed map of built providers allocates only the value it returns;
//! - a built graph's retained footprint is bounded per component and is fully reclaimed on drop.
//!
//! Allocation tracking is scoped to the measured test thread, so concurrent test-runner activity
//! cannot contaminate the results.

#[path = "../../src/test_support/alloc.rs"]
mod alloc;
mod graph;
mod traits;

use alloc::measure;

#[test]
fn measurement_ignores_allocations_from_other_threads() {
    use std::hint::{black_box, spin_loop};
    use std::sync::atomic::{AtomicBool, Ordering};

    let start = AtomicBool::new(false);
    let allocated = AtomicBool::new(false);

    std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            while !start.load(Ordering::Acquire) {
                spin_loop();
            }

            let allocation = Box::new([0u8; 64]);
            black_box(&allocation);
            allocated.store(true, Ordering::Release);

            allocation
        });

        let (_, delta) = measure(|| {
            start.store(true, Ordering::Release);
            while !allocated.load(Ordering::Acquire) {
                spin_loop();
            }
        });

        let allocation = worker.join().expect("allocation thread succeeds");
        assert_eq!(allocation.len(), 64, "background allocation was performed");
        assert_eq!(
            delta.allocations, 0,
            "another thread's allocation contaminated the measurement"
        );
        assert_eq!(
            delta.net_live, 0,
            "another thread's live bytes contaminated the measurement"
        );
    });
}
