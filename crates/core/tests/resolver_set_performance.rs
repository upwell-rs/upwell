use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use upwell_core::{Resolver, ResolverSet};

struct TrackingAllocator;

thread_local! {
    static TRACK_ALLOCATIONS: Cell<bool> = const { Cell::new(false) };
}

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every method forwards its arguments unchanged to `System`, so the contract the caller
// upholds is the one `System` requires. Counting only touches atomics and a thread-local.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count_allocation();

        // SAFETY: the caller's arguments, forwarded unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller's arguments, forwarded unchanged.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count_allocation();

        // SAFETY: the caller's arguments, forwarded unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count_allocation();

        // SAFETY: the caller's arguments, forwarded unchanged.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

fn count_allocation() {
    if TRACK_ALLOCATIONS.try_with(Cell::get).unwrap_or_default() {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    }
}

fn allocations_during(run: impl FnOnce()) -> usize {
    ALLOCATIONS.store(0, Ordering::SeqCst);
    TRACK_ALLOCATIONS.with(|tracking| tracking.set(true));
    run();
    TRACK_ALLOCATIONS.with(|tracking| tracking.set(false));

    ALLOCATIONS.load(Ordering::SeqCst)
}

struct TestResolver;
impl Resolver for TestResolver {}

/// This is a deterministic complexity contract, not a wall-clock benchmark: request-time
/// cloning must remain an `Arc` bump even as benchmark timing varies between CI hosts.
#[test]
fn cloning_a_resolver_set_is_allocation_free() {
    const CLONES: usize = 4_096;

    let mut set = ResolverSet::new();
    set.insert(Arc::new(TestResolver));
    let mut clones = Vec::with_capacity(CLONES);

    let allocations = allocations_during(|| {
        for _ in 0..CLONES {
            clones.push(set.clone());
        }
    });

    black_box(clones);
    assert_eq!(
        allocations, 0,
        "ResolverSet::clone allocated on the hot path"
    );
}
