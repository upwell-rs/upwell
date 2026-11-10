//! Inline and boxed stored values read, move and drop their value exactly once.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::Live;
use crate::test_support::measure;

/// Counts its drops, padded to `N` words.
struct Tracked<const N: usize> {
    drops: Arc<AtomicUsize>,
    _padding: [usize; N],
}

impl<const N: usize> Tracked<N> {
    fn new() -> (Self, Arc<AtomicUsize>) {
        let drops = Arc::new(AtomicUsize::new(0));
        let tracked = Self {
            drops: Arc::clone(&drops),
            _padding: [0; N],
        };

        (tracked, drops)
    }
}

impl<const N: usize> Drop for Tracked<N> {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

/// A value stricter than inline storage's alignment.
#[repr(align(32))]
struct OverAligned(u8);

/// Checks that a `Tracked<N>` stored in a [`StoredValue`] reads, moves and drops correctly.
fn exercises_value<const N: usize>() {
    let (tracked, drops) = Tracked::<N>::new();
    let stored = StoredValue::new(tracked);

    assert!(stored.is::<Tracked<N>>());
    assert!(
        stored.downcast_ref::<u8>().is_none(),
        "another type is refused"
    );

    let moved = stored;

    assert_eq!(
        drops.load(Ordering::SeqCst),
        0,
        "moving the storage drops nothing"
    );

    drop(moved);

    assert_eq!(
        drops.load(Ordering::SeqCst),
        1,
        "dropping the storage drops the value"
    );

    let (tracked, drops) = Tracked::<N>::new();
    let refused = StoredValue::new(tracked)
        .downcast::<u8>()
        .expect_err("another type is refused");

    assert_eq!(
        drops.load(Ordering::SeqCst),
        0,
        "a refused take keeps the value"
    );

    let taken = refused
        .downcast::<Tracked<N>>()
        .ok()
        .expect("the stored type is taken");

    assert_eq!(
        drops.load(Ordering::SeqCst),
        0,
        "taking moves the value out"
    );

    drop(taken);

    assert_eq!(
        drops.load(Ordering::SeqCst),
        1,
        "the taken value drops once"
    );
}

#[test]
fn an_inline_value_reads_moves_and_drops_once() {
    exercises_value::<2>();
}

#[test]
fn a_boxed_value_reads_moves_and_drops_once() {
    exercises_value::<8>();
}

#[test]
fn a_zero_sized_value_is_stored() {
    let stored = StoredValue::new(());

    assert!(stored.is::<()>());
    assert!(stored.downcast::<()>().is_ok());
}

#[test]
fn an_over_aligned_value_keeps_its_alignment() {
    let stored = StoredValue::new(OverAligned(7));
    let value = stored.downcast_ref::<OverAligned>().expect("stored");

    assert_eq!(value.0, 7);
    assert_eq!(
        std::ptr::from_ref(value).addr() % 32,
        0,
        "the value is aligned"
    );
}

#[test]
fn live_handles_are_stored_without_allocating() {
    let concrete = Live::fixed(Arc::new(1u8));
    let erased: Live<dyn Any + Send + Sync> = Live::fixed(Arc::new(1u8));

    let (stored, delta) = measure(|| (StoredValue::new(concrete), StoredValue::new(erased)));

    assert_eq!(
        delta.allocations, 0,
        "a live handle fits inline, wide pointer included"
    );
    assert!(stored.0.is::<Live<u8>>());
    assert!(stored.1.is::<Live<dyn Any + Send + Sync>>());
}

#[test]
fn an_inline_value_with_interior_mutability_is_written_through_a_shared_read() {
    let stored = StoredValue::new(AtomicUsize::new(1));
    let counter = stored
        .downcast_ref::<AtomicUsize>()
        .expect("the stored value is an AtomicUsize");

    counter.fetch_add(1, Ordering::SeqCst);

    assert_eq!(
        stored
            .downcast_ref::<AtomicUsize>()
            .expect("the stored value is still an AtomicUsize")
            .load(Ordering::SeqCst),
        2
    );
}
