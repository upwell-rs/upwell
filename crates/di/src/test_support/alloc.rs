//! Allocation measurement scoped to the calling thread.
//!
//! Shared by the crate's unit tests, its `memory_contracts` integration test and the protocols'
//! `allocation_contracts` tests. The counting allocator is `allocation-counter`'s, which each
//! test binary links by using it, so no allocator of our own is installed.
//!
//! The test harness runs bookkeeping on other threads while the test body executes. The counters
//! are thread-local, so those allocations cannot make a contract flaky, while every allocation
//! the code under measurement makes on the measured thread is observed.

/// The allocation traffic (calls and bytes requested) and net live-byte change across a
/// measured operation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Delta {
    pub(crate) allocations: u64,
    pub(crate) bytes: u64,
    pub(crate) net_live: i64,
}

/// Runs `f`, returning its result and what it allocated on this thread.
pub(crate) fn measure<R>(f: impl FnOnce() -> R) -> (R, Delta) {
    let mut value = None;
    let info = allocation_counter::measure(|| value = Some(f()));
    let delta = Delta {
        allocations: info.count_total,
        bytes: info.bytes_total,
        net_live: info.bytes_current,
    };

    (value.expect("the measured closure ran"), delta)
}
