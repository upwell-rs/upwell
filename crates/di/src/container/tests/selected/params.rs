//! Reading an entry's targets from the sources its plan pre-resolved for them.

use upwell_core::DependencyDescriptor;

use super::*;
use crate::FromContainer;

/// The dependency edge an entry injecting `H` declares.
fn target<H: FromContainer>() -> DependencyDescriptor {
    H::dependency()
}

/// The names of `greeters`, in order.
fn names(greeters: &[Arc<dyn Greeter>]) -> Vec<&'static str> {
    greeters.iter().map(|greeter| greeter.name()).collect()
}

#[tokio::test]
async fn targets_in_the_scope_and_the_root_read_their_planned_sources() {
    let generation = Generation::of(
        &[
            RootGreeter::descriptor(),
            InnerPrimary::descriptor(),
            InnerSpare::descriptor(),
        ],
        vec![
            RootGreeter::provider(),
            InnerPrimary::provider().primary(),
            InnerSpare::provider(),
        ],
        Reach::EveryChain,
    )
    .await;
    let plan = generation.plan_inner(&[
        target::<Arc<dyn Greeter>>(),
        target::<Vec<Arc<dyn Greeter>>>(),
        target::<HashMap<String, Arc<dyn Greeter>>>(),
        target::<Arc<InnerSpare>>(),
        target::<Dep<RootGreeter>>(),
    ]);

    let inner = generation.open_inner(&generation.root, Some(&plan)).await;
    let single = <Arc<dyn Greeter>>::read_planned(&inner, &plan).expect("the trait is planned");
    let all =
        <Vec<Arc<dyn Greeter>>>::read_planned(&inner, &plan).expect("the collection is planned");
    let keyed = <HashMap<String, Arc<dyn Greeter>>>::read_planned(&inner, &plan)
        .expect("the keyed map is planned");
    let spare = <Arc<InnerSpare>>::read_planned(&inner, &plan).expect("the local slot is planned");
    let root = <Dep<RootGreeter>>::read_planned(&inner, &plan).expect("the singleton is planned");
    let extracted = inner
        .extract_planned::<Arc<dyn Greeter>>(Some(&plan))
        .await
        .expect("the planned trait extracts");

    assert_eq!(single.name(), "inner-primary");
    assert_eq!(names(&all), ["root", "inner-primary", "inner-spare"]);
    assert_eq!(
        keyed.get("inner-spare").map(|greeter| greeter.name()),
        Some("inner-spare")
    );
    assert_eq!(spare.name(), "inner-spare");
    assert_eq!(root.snapshot().name(), "root");
    assert!(std::ptr::addr_eq(
        Arc::as_ptr(&extracted),
        Arc::as_ptr(&single)
    ));
}

#[tokio::test]
async fn a_target_the_plan_does_not_inject_has_no_parameter() {
    let generation = Generation::of(
        &[RootGreeter::descriptor(), InnerPrimary::descriptor()],
        vec![RootGreeter::provider(), InnerPrimary::provider()],
        Reach::EveryChain,
    )
    .await;
    let plan = generation.plan_inner(&[target::<Arc<InnerPrimary>>()]);

    let inner = generation.open_inner(&generation.root, Some(&plan)).await;

    assert!(<Arc<RootGreeter>>::read_planned(&inner, &plan).is_none());
    assert_eq!(
        inner
            .extract_planned::<Arc<RootGreeter>>(Some(&plan))
            .await
            .expect("an unplanned target extracts as any other")
            .name(),
        "root"
    );
}

#[tokio::test]
async fn an_enclosing_scope_target_is_extracted_as_any_other() {
    let generation = Generation::of(
        &[
            RootGreeter::descriptor(),
            OuterGreeter::descriptor(),
            InnerAnchor::descriptor(),
        ],
        vec![RootGreeter::provider(), OuterGreeter::provider()],
        Reach::Outer,
    )
    .await;
    let plan =
        generation.plan_inner(&[target::<Arc<OuterGreeter>>(), target::<Arc<dyn Greeter>>()]);

    let outer = generation.open_outer().await;
    let inner = generation.open_inner(&outer, Some(&plan)).await;

    assert!(
        <Arc<OuterGreeter>>::read_planned(&inner, &plan).is_none(),
        "a carried plan could name a stale slot of an enclosing layout"
    );
    assert!(<Arc<dyn Greeter>>::read_planned(&inner, &plan).is_none());
    assert_eq!(
        inner
            .extract_planned::<Arc<dyn Greeter>>(Some(&plan))
            .await
            .expect("the enclosing provider extracts")
            .name(),
        "outer"
    );
}

#[tokio::test]
async fn a_parent_off_some_chains_shadows_a_planned_root_provider() {
    let generation = Generation::of(
        &[
            RootGreeter::descriptor(),
            OuterGreeter::descriptor(),
            InnerAnchor::descriptor(),
        ],
        vec![RootGreeter::provider(), OuterGreeter::provider()],
        Reach::EveryChain,
    )
    .await;
    let plan = generation.plan_inner(&[target::<Arc<dyn Greeter>>()]);

    let outer = generation.open_outer().await;
    let under_root = generation.open_inner(&generation.root, Some(&plan)).await;
    let under_outer = generation.open_inner(&outer, Some(&plan)).await;

    assert_eq!(
        <Arc<dyn Greeter>>::read_planned(&under_root, &plan).map(|greeter| greeter.name()),
        Some("root")
    );
    assert!(<Arc<dyn Greeter>>::read_planned(&under_outer, &plan).is_none());
    assert_eq!(
        under_outer
            .extract_planned::<Arc<dyn Greeter>>(Some(&plan))
            .await
            .expect("the shadowing provider extracts")
            .name(),
        "outer"
    );
}

#[tokio::test]
async fn the_scope_an_instance_opens_under_reads_only_sources_outside_it() {
    let generation = Generation::of(
        &[RootGreeter::descriptor(), InnerPrimary::descriptor()],
        vec![RootGreeter::provider(), InnerPrimary::provider()],
        Reach::EveryChain,
    )
    .await;
    let plan =
        generation.plan_inner(&[target::<Arc<RootGreeter>>(), target::<Arc<InnerPrimary>>()]);

    assert_eq!(
        <Arc<RootGreeter>>::read_planned(&generation.root, &plan).map(|greeter| greeter.name()),
        Some("root")
    );
    assert!(
        <Arc<InnerPrimary>>::read_planned(&generation.root, &plan).is_none(),
        "a slot of the plan's own scope is read only from that scope"
    );
}

#[tokio::test]
async fn an_optional_target_nothing_provides_reads_as_absent() {
    let generation = Generation::of(
        &[RootGreeter::descriptor(), InnerPrimary::descriptor()],
        vec![RootGreeter::provider(), InnerPrimary::provider()],
        Reach::EveryChain,
    )
    .await;
    let plan = generation.plan_inner(&[target::<Option<Arc<OuterAnchor>>>()]);

    let inner = generation.open_inner(&generation.root, Some(&plan)).await;

    assert_eq!(
        <Option<Arc<OuterAnchor>>>::read_planned(&inner, &plan).map(|anchor| anchor.is_none()),
        Some(true)
    );
}

#[tokio::test]
async fn planning_the_same_targets_twice_plans_equal_parameters() {
    let generation = Generation::of(
        &[RootGreeter::descriptor(), InnerPrimary::descriptor()],
        vec![RootGreeter::provider(), InnerPrimary::provider().primary()],
        Reach::EveryChain,
    )
    .await;
    let targets = [
        target::<Arc<dyn Greeter>>(),
        target::<Vec<Arc<dyn Greeter>>>(),
        target::<Arc<RootGreeter>>(),
    ];

    assert_eq!(
        generation.plan_inner(&targets),
        generation.plan_inner(&targets)
    );
}
