//! A deferred cycle left out of a scope's opening, built on demand as one group.

use std::sync::atomic::AtomicBool;

use super::*;
use crate::Deferred;

/// Half of a cycle, reaching [`Back`] once its scope hydrates it.
struct Forth {
    back: Deferred<Back>,
}

/// The other half of the cycle, depending on [`Forth`] directly.
struct Back {
    forth: Arc<Forth>,
}

test_component!(Forth, "forth");
test_component!(Back, "back");

async fn forth(back: Deferred<Back>) -> Forth {
    Forth { back }
}

async fn back(forth: Arc<Forth>) -> Back {
    Back { forth }
}

scoped!(forth_descriptor, Forth, "forth", forth);
scoped!(back_descriptor, Back, "back", back);

fn cycle_layout() -> Arc<crate::SlotLayout> {
    Arc::new(plan(&[forth_descriptor(), back_descriptor()]))
}

/// Checks that `forth` and `back` are the two halves of one cycle built in `scope`.
fn assert_one_cycle(scope: &Arc<ScopeContainer>, forth: &Arc<Forth>) {
    let back = scope
        .get::<Back>()
        .expect("the cycle's other half is built");

    assert!(Arc::ptr_eq(&back.forth, forth));
    assert!(Arc::ptr_eq(&forth.back.get(), &back));
}

#[tokio::test]
async fn an_entry_reaching_one_half_of_a_deferred_cycle_builds_both() {
    let layout = cycle_layout();
    let plan = entry_plan(&layout, &[<Arc<Forth> as FromContainer>::dependency()]);

    let scope = open(&layout, Some(&plan)).await;
    let forth = scope.get::<Forth>().expect("the targeted half is built");

    assert_eq!(plan.len(), 2);
    assert_one_cycle(&scope, &forth);
}

#[tokio::test]
async fn a_deferred_cycle_left_out_of_the_opening_is_built_on_demand_as_a_unit() {
    let layout = cycle_layout();
    let scope = open(&layout, Some(&entry_plan(&layout, &[]))).await;

    let forth = scope
        .resolve::<Arc<Forth>>()
        .await
        .expect("the cycle builds on demand")
        .expect("the targeted half is built");

    assert_one_cycle(&scope, &forth);
}

/// Whether [`Tail`] may finish building.
static TAIL_OPEN: AtomicBool = AtomicBool::new(false);

/// Signalled when a [`Tail`] build starts while [`TAIL_OPEN`] is unset.
static TAIL_STARTED: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Half of a cycle whose other half, [`Tail`], may be left unbuilt by a cancelled build.
struct Head {
    tail: Deferred<Tail>,
}

/// The other half of the cycle, whose build waits until [`TAIL_OPEN`] is set.
struct Tail {
    head: Arc<Head>,
}

test_component!(Head, "head");
test_component!(Tail, "tail");

async fn head(tail: Deferred<Tail>) -> Head {
    Head { tail }
}

async fn tail(head: Arc<Head>) -> Tail {
    if !TAIL_OPEN.load(Ordering::SeqCst) {
        TAIL_STARTED.notify_one();
        std::future::pending::<()>().await;
    }

    Tail { head }
}

scoped!(head_descriptor, Head, "head", head);
scoped!(tail_descriptor, Tail, "tail", tail);

#[tokio::test]
async fn a_cycle_half_built_by_a_cancelled_build_reads_its_other_half_once_built() {
    let layout = Arc::new(plan(&[head_descriptor(), tail_descriptor()]));
    let scope = open(&layout, Some(&entry_plan(&layout, &[]))).await;
    let cancelled = tokio::spawn({
        let scope = Arc::clone(&scope);

        async move { scope.resolve::<Arc<Head>>().await }
    });

    TAIL_STARTED.notified().await;
    cancelled.abort();

    let aborted = cancelled.await;
    let head = scope
        .get::<Head>()
        .expect("the head was built before the cancellation");

    TAIL_OPEN.store(true, Ordering::SeqCst);

    let tail = scope
        .resolve::<Arc<Tail>>()
        .await
        .expect("the tail builds on a later resolution")
        .expect("the tail is built");

    assert!(aborted.is_err_and(|error| error.is_cancelled()));
    assert!(Arc::ptr_eq(&tail.head, &head));
    assert!(
        Arc::ptr_eq(&head.tail.get(), &tail),
        "the head reads its deferred tail once the tail is built"
    );
}

/// Holds a factory until the test releases it, telling the test when the factory started.
struct Gate {
    started: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

impl Gate {
    const fn new() -> Self {
        Self {
            started: tokio::sync::Notify::const_new(),
            release: tokio::sync::Notify::const_new(),
        }
    }

    async fn pass(&self) {
        self.started.notify_one();
        self.release.notified().await;
    }
}

static SPOKE_GATE: Gate = Gate::new();
static RIM_GATE: Gate = Gate::new();

/// Half of a cycle, reaching [`Spoke`] once its scope hydrates it.
struct Hub {
    spoke: Deferred<Spoke>,
}

/// The other half of the cycle, built once [`SPOKE_GATE`] releases it.
struct Spoke {
    hub: Arc<Hub>,
}

/// A slot after the cycle that only some resolutions build, held by [`RIM_GATE`].
struct Rim {
    _hub: Arc<Hub>,
}

test_component!(Hub, "hub");
test_component!(Spoke, "spoke");
test_component!(Rim, "rim");

async fn hub(spoke: Deferred<Spoke>) -> Hub {
    Hub { spoke }
}

async fn spoke(hub: Arc<Hub>) -> Spoke {
    SPOKE_GATE.pass().await;

    Spoke { hub }
}

async fn rim(hub: Arc<Hub>) -> Rim {
    RIM_GATE.pass().await;

    Rim { _hub: hub }
}

scoped!(hub_descriptor, Hub, "hub", hub);
scoped!(spoke_descriptor, Spoke, "spoke", spoke);
scoped!(rim_descriptor, Rim, "rim", rim);

#[tokio::test]
async fn a_cycle_member_returned_while_another_group_still_builds_reads_its_other_half() {
    let layout = Arc::new(plan(&[
        hub_descriptor(),
        spoke_descriptor(),
        rim_descriptor(),
    ]));
    let scope = open(&layout, Some(&entry_plan(&layout, &[]))).await;
    let building = tokio::spawn({
        let scope = Arc::clone(&scope);

        async move { scope.resolve::<Arc<Rim>>().await.map(|rim| rim.is_some()) }
    });

    SPOKE_GATE.started.notified().await;

    let reading = tokio::spawn({
        let scope = Arc::clone(&scope);

        async move { scope.resolve::<Arc<Hub>>().await }
    });

    SPOKE_GATE.release.notify_one();
    RIM_GATE.started.notified().await;

    let hub = reading
        .await
        .expect("the reading task finishes")
        .expect("the hub resolves")
        .expect("the hub is built");
    let spoke = scope.get::<Spoke>().expect("the spoke is built");
    let read = hub.spoke.try_get();

    RIM_GATE.release.notify_one();

    let built = building.await.expect("the building task finishes");

    assert!(
        read.is_some_and(|read| Arc::ptr_eq(&read, &spoke)),
        "the hub reads its spoke before the group that built them hydrates it"
    );
    assert!(Arc::ptr_eq(&spoke.hub, &hub));
    assert!(built.is_ok_and(|built| built));
}
