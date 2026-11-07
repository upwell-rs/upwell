//! Seeds an opening leaves out: which openings read them, and filling them later.

use std::sync::atomic::AtomicUsize;

use super::*;
use crate::{Injectable, Lazy};
use upwell_core::TypeDescriptor;

/// A by-value seed.
#[derive(Clone, Debug, PartialEq)]
struct Stamp(u8);

impl Injectable for Stamp {
    type Target = Self;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

/// A component reading [`Stamp`].
struct Reader {
    stamp: Stamp,
}

/// A component reaching [`Reader`] lazily, so it reads [`Stamp`] once resolved.
struct LazyReader;

test_component!(Reader, "reader");
test_component!(LazyReader, "lazy_reader");

async fn reader(stamp: Stamp) -> Reader {
    Reader { stamp }
}

async fn lazy_reader(_reader: Lazy<Arc<Reader>>) -> LazyReader {
    LazyReader
}

scoped!(reader_descriptor, Reader, "reader", reader);
scoped!(
    lazy_reader_descriptor,
    LazyReader,
    "lazy_reader",
    lazy_reader
);

fn stamp_descriptor() -> ComponentDescriptor {
    ComponentDescriptor::manual(
        "stamp",
        "Stamp",
        TypeDescriptor::of::<Stamp>(std::any::type_name::<Stamp>()),
        TestScope::KEY,
    )
}

fn seeded_layout() -> Arc<crate::SlotLayout> {
    Arc::new(plan(&[
        stamp_descriptor(),
        reader_descriptor(),
        leaf_descriptor(),
    ]))
}

fn stamp_id() -> TypeId {
    TypeId::of::<Stamp>()
}

/// Runs `source`, counting the calls in `calls`.
fn counted(calls: &AtomicUsize) -> impl FnOnce() -> Stamp + '_ {
    || {
        calls.fetch_add(1, Ordering::SeqCst);

        Stamp(7)
    }
}

#[test]
fn an_entry_reads_a_seed_it_injects_or_reaches_eagerly_or_lazily() {
    let layout = seeded_layout();
    let reads = |target: DependencyDescriptor| {
        entry_plan(&layout, &[target]).reads_seed(&layout, stamp_id())
    };

    assert!(reads(<Stamp as FromContainer>::dependency()));
    assert!(reads(<Arc<Reader> as FromContainer>::dependency()));
    assert!(reads(<Lazy<Arc<Reader>> as FromContainer>::dependency()));
    assert!(!reads(<Arc<Leaf> as FromContainer>::dependency()));
    assert!(!entry_plan(&layout, &[]).reads_seed(&layout, stamp_id()));
}

#[test]
fn an_entry_reads_a_seed_through_a_lazy_edge_of_what_it_builds() {
    let layout = Arc::new(plan(&[
        stamp_descriptor(),
        reader_descriptor(),
        lazy_reader_descriptor(),
    ]));
    let plan = entry_plan(&layout, &[<Arc<LazyReader> as FromContainer>::dependency()]);

    assert!(plan.reads_seed(&layout, stamp_id()));
}

#[test]
fn a_layout_reports_which_openings_read_a_seed() {
    let read = seeded_layout();
    let unread = Arc::new(plan(&[stamp_descriptor(), leaf_descriptor()]));
    let mut eager_reader = reader_descriptor();

    eager_reader.construction = crate::ScopedConstruction::Eager;

    let eager = Arc::new(plan(&[stamp_descriptor(), eager_reader]));
    let seeds = read.seeds().map(|seed| seed.id).collect::<Vec<_>>();

    assert_eq!(seeds, ["stamp"]);
    assert!(read.factories_read_seed(stamp_id()));
    assert!(!read.eager_set_reads_seed(stamp_id()));
    assert!(!unread.factories_read_seed(stamp_id()));
    assert!(eager.eager_set_reads_seed(stamp_id()));
    assert!(
        !read.factories_read_seed(TypeId::of::<Leaf>()) && read.seeds().count() == 1,
        "a slot nothing depends on is read by no factory"
    );
}

#[tokio::test]
async fn extraction_fills_a_seed_only_when_what_it_resolves_reads_it() {
    let layout = seeded_layout();
    let plan = entry_plan(&layout, &[]);
    let scope = open(&layout, Some(&plan)).await;
    let calls = AtomicUsize::new(0);

    scope
        .extract_seeding::<Arc<Leaf>, Stamp>(counted(&calls))
        .await
        .expect("the leaf resolves");

    assert_eq!(calls.load(Ordering::SeqCst), 0, "the leaf reads no seed");

    let reader = scope
        .extract_seeding::<Arc<Reader>, Stamp>(counted(&calls))
        .await
        .expect("the reader resolves with the seed filled for it");
    let stamp = scope
        .extract_seeding::<Stamp, Stamp>(counted(&calls))
        .await
        .expect("the seed reads back");

    assert_eq!(reader.stamp, Stamp(7));
    assert_eq!(stamp, Stamp(7));
    assert_eq!(calls.load(Ordering::SeqCst), 1, "one seed per scope");
}

#[tokio::test]
async fn extraction_of_a_lazy_handle_fills_the_seed_it_resolves_later() {
    let layout = seeded_layout();
    let plan = entry_plan(&layout, &[]);
    let scope = open(&layout, Some(&plan)).await;
    let calls = AtomicUsize::new(0);

    let lazy = scope
        .extract_seeding::<Lazy<Arc<Reader>>, Stamp>(counted(&calls))
        .await
        .expect("the handle resolves");

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        lazy.get_or_create().await.expect("the reader builds").stamp,
        Stamp(7)
    );
}

#[tokio::test]
async fn a_scope_of_seeds_alone_opened_without_them_fills_them_later() {
    let layout = Arc::new(plan(&[stamp_descriptor()]));
    let scope = open(&layout, None).await;

    assert!(
        scope
            .resolve::<Stamp>()
            .await
            .expect("nothing fails")
            .is_none()
    );

    scope.ensure_seed(|| Stamp(3)).expect("the slot is a seed");
    scope
        .ensure_seed(|| -> Stamp { panic!("the slot holds its value") })
        .expect("a filled slot is left alone");

    assert_eq!(
        scope.extract::<Stamp>().await.expect("the seed reads"),
        Stamp(3)
    );
}

#[tokio::test]
async fn filling_a_seed_ignores_a_type_without_a_slot_and_rejects_a_built_one() {
    let layout = seeded_layout();
    let scope = open(&layout, Some(&entry_plan(&layout, &[]))).await;

    scope
        .ensure_seed(|| Other)
        .expect("a type without a slot is left alone");

    assert!(matches!(
        scope.ensure_seed(|| Leaf),
        Err(crate::error::Error::SeedForBuiltComponent { .. })
    ));
}

#[tokio::test]
async fn filling_a_seed_is_not_a_construction_a_strict_entry_must_declare() {
    let layout = seeded_layout();
    let plan = entry_plan_under(&layout, &[], UnplannedConstruction::Deny);
    let scope = open(&layout, Some(&plan)).await;

    let stamp = scope
        .extract_seeding::<Stamp, Stamp>(|| Stamp(9))
        .await
        .expect("a seed is filled, not built");

    assert_eq!(stamp, Stamp(9));
}
