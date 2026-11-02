//! A global allocator that measures allocations on the calling thread.
//!
//! Shared by the crate's unit tests and its `memory_contracts` integration test, each of which
//! installs it for its own test binary.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

/// The allocation traffic and net live-byte change across a measured operation.
#[derive(Clone, Copy, Default)]
pub(crate) struct Delta {
    pub(crate) allocations: u64,
    pub(crate) net_live: i64,
}

thread_local! {
    /// Allocator activity in the current thread's active measurement, if any.
    ///
    /// The test harness runs bookkeeping on other threads while the test body executes. Keeping
    /// this state thread-local prevents those unrelated allocations from making the contracts
    /// flaky while still observing every allocation made by the code under measurement.
    static MEASUREMENT: Cell<Option<Delta>> = const { Cell::new(None) };
}

fn record_allocation(allocations: u64, net_live: i64) {
    // Allocation can occur while a thread's TLS is being destroyed, when `with` would panic. Such
    // activity is outside a measurement, so it is safe to ignore an unavailable tracker.
    let _ = MEASUREMENT.try_with(|measurement| {
        if let Some(mut delta) = measurement.get() {
            delta.allocations += allocations;
            delta.net_live += net_live;
            measurement.set(Some(delta));
        }
    });
}

/// Forwards to the system allocator, recording what the measured thread allocates.
struct TrackingAllocator;

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation(1, layout.size() as i64);

        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        record_allocation(0, -(layout.size() as i64));

        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_allocation(1, layout.size() as i64);

        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record_allocation(1, new_size as i64 - layout.size() as i64);

        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

/// Runs `f`, returning its result and what it allocated on this thread.
pub(crate) fn measure<R>(f: impl FnOnce() -> R) -> (R, Delta) {
    MEASUREMENT.with(|measurement| {
        assert!(
            measurement.replace(Some(Delta::default())).is_none(),
            "memory measurements must not be nested"
        );
    });

    let value = f();
    let delta = MEASUREMENT.with(|measurement| {
        measurement
            .replace(None)
            .expect("memory measurement was active")
    });

    (value, delta)
}
