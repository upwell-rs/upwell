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
