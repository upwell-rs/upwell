//! A tracking global allocator for measuring heap behaviour in benchmarks.
//!
//! [`TrackingAllocator`] forwards every request to the system allocator while recording three
//! running totals: the number of allocations, the cumulative bytes requested (allocation
//! *traffic*), and the currently *live* bytes (`alloc − dealloc`). Traffic drives the Criterion
//! memory [`measure`](crate::measure)ment; live bytes let a test prove a hot path returns to its
//! baseline (i.e. leaks nothing).
//!
//! A bench opts in by installing it as the process `#[global_allocator]`:
//!
//! ```ignore
//! #[global_allocator]
//! static GLOBAL: upwell_benchmarks::alloc::TrackingAllocator =
//!     upwell_benchmarks::alloc::TrackingAllocator;
//! ```
//!
//! Benches that measure only wall-clock time do **not** install it, so their timing is never
//! perturbed by the counters.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static BYTES_ALLOCATED: AtomicU64 = AtomicU64::new(0);
static LIVE_BYTES: AtomicI64 = AtomicI64::new(0);

/// A [`GlobalAlloc`] wrapper over [`System`] that records allocation count, cumulative bytes, and
/// live bytes. Zero-sized: install one `static` of it as the global allocator.
pub struct TrackingAllocator;

// SAFETY: every method forwards its arguments unchanged to `System`, so the contract the caller
// upholds is the one `System` requires. Recording only updates atomic counters.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's arguments, forwarded unchanged.
        let ptr = unsafe { System.alloc(layout) };

        if !ptr.is_null() {
            record_alloc(layout.size());
        }

        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE_BYTES.fetch_sub(layout.size() as i64, Ordering::Relaxed);

        // SAFETY: the caller's arguments, forwarded unchanged.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's arguments, forwarded unchanged.
        let ptr = unsafe { System.alloc_zeroed(layout) };

        if !ptr.is_null() {
            record_alloc(layout.size());
        }

        ptr
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller's arguments, forwarded unchanged.
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };

        // A successful realloc hands out a fresh `new_size`-byte buffer, so it is `new_size` bytes of
        // traffic (the old buffer was counted at its own allocation and is now freed). Live bytes
        // move by the net delta. A failed realloc (`null`) leaves the original allocation intact, so
        // nothing changes. Counting only the delta here would hide buffer-growth churn — exactly the
        // traffic this metric exists to surface.
        if !new_ptr.is_null() {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            BYTES_ALLOCATED.fetch_add(new_size as u64, Ordering::Relaxed);
            LIVE_BYTES.fetch_add(new_size as i64 - layout.size() as i64, Ordering::Relaxed);
        }

        new_ptr
    }
}

fn record_alloc(size: usize) {
    ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    BYTES_ALLOCATED.fetch_add(size as u64, Ordering::Relaxed);
    LIVE_BYTES.fetch_add(size as i64, Ordering::Relaxed);
}

/// A snapshot of the three running allocation counters.
#[derive(Clone, Copy, Debug)]
pub struct Snapshot {
    /// Total allocation calls (`alloc*` plus `realloc`) recorded so far.
    pub allocations: u64,
    /// Cumulative bytes requested so far — allocation *traffic*, never decreasing.
    pub bytes_allocated: u64,
    /// Currently outstanding bytes (`alloc − dealloc`); returns to its baseline once every
    /// allocation made in a region is freed.
    pub live_bytes: i64,
}

/// Reads the current counters. Cheap (three relaxed loads); safe to call anywhere.
pub fn snapshot() -> Snapshot {
    Snapshot {
        allocations: ALLOCATIONS.load(Ordering::Relaxed),
        bytes_allocated: BYTES_ALLOCATED.load(Ordering::Relaxed),
        live_bytes: LIVE_BYTES.load(Ordering::Relaxed),
    }
}

/// The cumulative bytes-allocated counter — the value the Criterion memory measurement samples.
pub fn bytes_allocated() -> u64 {
    BYTES_ALLOCATED.load(Ordering::Relaxed)
}

/// The currently live (un-freed) byte count.
pub fn live_bytes() -> i64 {
    LIVE_BYTES.load(Ordering::Relaxed)
}

/// Runs `f`, returning its result alongside the allocation traffic (count and bytes) it caused and
/// the *net* change in live bytes across the call. A net-live of `0` means everything `f` allocated
/// was also freed — the property a leak test asserts.
pub fn measure<R>(f: impl FnOnce() -> R) -> (R, Delta) {
    let before = snapshot();
    let value = f();
    let after = snapshot();

    let delta = Delta {
        allocations: after.allocations - before.allocations,
        bytes_allocated: after.bytes_allocated - before.bytes_allocated,
        net_live_bytes: after.live_bytes - before.live_bytes,
    };

    (value, delta)
}

/// The change in each counter across a [`measure`]d region.
#[derive(Clone, Copy, Debug)]
pub struct Delta {
    /// Allocation calls made during the region.
    pub allocations: u64,
    /// Bytes requested during the region.
    pub bytes_allocated: u64,
    /// Net change in live bytes; `0` iff the region freed everything it allocated.
    pub net_live_bytes: i64,
}
