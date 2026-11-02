use super::*;

fn fixed() -> (Arc<u32>, Live<u32>) {
    let value = Arc::new(7);

    (Arc::clone(&value), Live::fixed(value))
}

#[test]
fn a_fixed_slot_reads_its_instance_without_a_cell() {
    let (value, live) = fixed();

    assert!(Arc::ptr_eq(&live.snapshot(), &value));
    assert_eq!(*live.get(), 7);
    assert!(Arc::ptr_eq(&live.clone().snapshot(), &value));
    assert!(
        <Arc<u32> as Injectable>::live_cell(&live).is_none(),
        "no transition can republish a fixed slot"
    );
}

#[test]
fn a_dep_read_from_a_fixed_slot_gets_its_own_cell() {
    let (value, live) = fixed();

    let dep = <Dep<u32> as Injectable>::from_stored(&live);

    assert!(Arc::ptr_eq(&dep.snapshot(), &value));

    dep.replace(Arc::new(8));

    assert_eq!(*dep.get(), 8, "the handle's own cell takes the replacement");
    assert!(
        Arc::ptr_eq(&live.snapshot(), &value),
        "the stored slot keeps its instance"
    );
}

#[test]
fn a_swappable_slot_shares_its_cell() {
    let live = Live::new(Arc::new(7));

    let dep = <Dep<u32> as Injectable>::from_stored(&live);

    dep.replace(Arc::new(8));

    assert_eq!(*live.get(), 8);
    assert!(<Arc<u32> as Injectable>::live_cell(&live).is_some());
}

#[test]
fn an_alias_keeps_the_storage_kind_of_its_instance() {
    let (_, fixed) = fixed();
    let swappable = Live::new(Arc::new(7));

    assert!(
        <Arc<str> as Injectable>::live_cell(&fixed.with_value::<str>(Arc::from("fixed"))).is_none()
    );
    assert!(
        <Arc<str> as Injectable>::live_cell(&swappable.with_value::<str>(Arc::from("live")))
            .is_some()
    );
}

/// A root slot holding `7`, a consumer's `Dep` sharing its cell, and the cell's rebind to `8`.
fn rebound_root() -> (Live<u32>, Dep<u32>, Arc<u32>) {
    let built = Arc::new(7);
    let root = <Arc<u32> as Injectable>::into_stored(Arc::clone(&built));
    let consumer = <Dep<u32> as Injectable>::from_stored(&root);
    let held = <Arc<u32> as Injectable>::live_cell(&root).expect("a root slot has a cell");
    let next = LiveCell::of(&Live::new(Arc::new(8_u32)));

    held.rebind_to(&next)
        .expect("both cells hold the same type")
        .commit();

    (root, consumer, built)
}

#[test]
fn a_rebound_root_slot_keeps_its_own_instance() {
    let (root, consumer, built) = rebound_root();

    assert!(
        Arc::ptr_eq(&<Arc<u32> as Injectable>::from_stored(&root), &built),
        "reads through the displaced root stay on its generation"
    );
    assert_eq!(*consumer.get(), 8, "the consumer's Dep follows the rebind");
    assert_eq!(
        *<Dep<u32> as Injectable>::from_stored(&root).get(),
        8,
        "a Dep read from the root shares the rebound cell"
    );
}

#[test]
fn a_root_slot_follows_a_replace_until_a_transition_rebinds_it() {
    let root = <Arc<u32> as Injectable>::into_stored(Arc::new(7));
    let dep = <Dep<u32> as Injectable>::from_stored(&root);

    dep.replace(Arc::new(9));

    assert_eq!(*<Arc<u32> as Injectable>::from_stored(&root), 9);
}

#[test]
fn a_rebound_root_releases_its_instance_with_the_root() {
    let (root, consumer, built) = rebound_root();
    let released = Arc::downgrade(&built);

    drop(built);
    drop(root);

    assert!(
        released.upgrade().is_none(),
        "the consumer's cell does not keep the displaced instance alive"
    );
    assert_eq!(*consumer.get(), 8);
}
