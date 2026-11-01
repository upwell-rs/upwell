//! The selection rules a layout's selected sources must agree with runtime selection on.

use super::*;

#[tokio::test]
async fn a_local_primary_is_read_from_its_slot() {
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

    let inner = generation.open_inner(&generation.root, None).await;
    let optional = inner
        .extract::<Option<Arc<dyn Greeter>>>()
        .await
        .expect("the optional greeter extracts");
    let live = inner
        .extract::<Dep<dyn Greeter>>()
        .await
        .expect("the live greeter extracts");

    assert_eq!(read_selected(&inner, None), Some("inner-primary"));
    assert_eq!(extracted(&inner).await, "inner-primary");
    assert_eq!(
        optional.map(|greeter| greeter.name()),
        Some("inner-primary")
    );
    assert_eq!(live.snapshot().name(), "inner-primary");
}

#[tokio::test]
async fn an_ambiguous_scope_falls_through_to_the_next_one() {
    let generation = Generation::of(
        &[
            RootGreeter::descriptor(),
            InnerPrimary::descriptor(),
            InnerSpare::descriptor(),
        ],
        vec![
            RootGreeter::provider(),
            InnerPrimary::provider(),
            InnerSpare::provider(),
        ],
        Reach::EveryChain,
    )
    .await;

    let inner = generation.open_inner(&generation.root, None).await;

    assert_eq!(read_selected(&inner, None), Some("root"));
    assert_eq!(extracted(&inner).await, "root");
}

#[tokio::test]
async fn a_sole_root_provider_is_read_under_either_parent() {
    let generation = Generation::of(
        &[
            RootGreeter::descriptor(),
            OuterAnchor::descriptor(),
            InnerAnchor::descriptor(),
        ],
        vec![RootGreeter::provider()],
        Reach::EveryChain,
    )
    .await;

    let outer = generation.open_outer().await;
    let under_root = generation.open_inner(&generation.root, None).await;
    let under_outer = generation.open_inner(&outer, None).await;

    assert_eq!(read_selected(&under_root, None), Some("root"));
    assert_eq!(read_selected(&under_outer, None), Some("root"));
    assert_eq!(extracted(&under_outer).await, "root");
}

#[tokio::test]
async fn a_parent_off_some_chains_shadows_the_planned_provider() {
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

    let outer = generation.open_outer().await;
    let under_root = generation.open_inner(&generation.root, None).await;
    let under_outer = generation.open_inner(&outer, None).await;

    assert_eq!(read_selected(&under_root, None), Some("root"));
    assert_eq!(extracted(&under_root).await, "root");
    assert_eq!(
        read_selected(&under_outer, None),
        None,
        "the outer provider shadows the root one the layout planned"
    );
    assert_eq!(
        selected_at_runtime(&under_outer, None).map(|greeter| greeter.name()),
        Some("outer")
    );
    assert_eq!(extracted(&under_outer).await, "outer");
}

#[tokio::test]
async fn an_enclosing_provider_is_read_from_its_layout() {
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

    let outer = generation.open_outer().await;
    let under_outer = generation.open_inner(&outer, None).await;
    let under_root = generation.open_inner(&generation.root, None).await;

    assert_eq!(read_selected(&under_outer, None), Some("outer"));
    assert_eq!(extracted(&under_outer).await, "outer");
    assert_eq!(
        read_selected(&under_root, None),
        None,
        "the planned enclosing scope is not on this chain"
    );
    assert_eq!(extracted(&under_root).await, "root");
}

#[tokio::test]
async fn an_enclosing_scope_opened_without_its_layout_selects_at_runtime() {
    let generation = Generation::of(
        &[OuterGreeter::descriptor(), InnerAnchor::descriptor()],
        vec![OuterGreeter::provider()],
        Reach::Outer,
    )
    .await;

    let outer = ScopeContainer::open_child(
        Outer::KEY,
        Arc::clone(&generation.root),
        Arc::clone(&generation.registry),
        &[OuterGreeter::descriptor()],
        Vec::new(),
    )
    .await
    .expect("the outer scope opens from its construction order");
    let inner = generation.open_inner(&outer, None).await;

    assert_eq!(read_selected(&inner, None), None);
    assert_eq!(extracted(&inner).await, "outer");
}

#[tokio::test]
async fn an_unbuilt_provider_is_selected_and_built_then_read() {
    let generation = Generation::of(
        &[InnerPrimary::descriptor()],
        vec![InnerPrimary::provider()],
        Reach::EveryChain,
    )
    .await;

    let inner = generation
        .open_inner(
            &generation.root,
            Some(&EntryPlan::on_demand(&generation.inner)),
        )
        .await;

    assert_eq!(read_selected(&inner, None), None);
    assert_eq!(extracted(&inner).await, "inner-primary");
    assert_eq!(read_selected(&inner, None), Some("inner-primary"));
}

#[tokio::test]
async fn a_transient_provider_is_never_read_from_a_slot() {
    let generation = Generation::of(
        &[
            RootGreeter::descriptor(),
            FreshGreeter::descriptor(),
            InnerAnchor::descriptor(),
        ],
        vec![RootGreeter::provider(), FreshGreeter::provider().primary()],
        Reach::EveryChain,
    )
    .await;

    let inner = generation.open_inner(&generation.root, None).await;

    assert!(
        generation
            .inner
            .selected(TypeId::of::<dyn Greeter>(), None)
            .is_none(),
        "a transient selection keeps runtime selection"
    );
    assert_eq!(read_selected(&inner, None), None);
    assert_eq!(extracted(&inner).await, "fresh");
}

#[tokio::test]
async fn a_qualifier_selects_its_provider() {
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

    let inner = generation.open_inner(&generation.root, None).await;
    let resolved = inner
        .resolve_qualified::<Arc<dyn Greeter>>("inner-spare")
        .await
        .expect("the qualified greeter resolves")
        .expect("a provider is qualified `inner-spare`");

    assert_eq!(
        read_selected(&inner, Some("inner-spare")),
        Some("inner-spare")
    );
    assert_eq!(read_selected(&inner, Some("root")), Some("root"));
    assert_eq!(read_selected(&inner, Some("missing")), None);
    assert_eq!(resolved.name(), "inner-spare");
}

#[tokio::test]
async fn provider_order_picks_between_providers_sharing_a_qualifier() {
    let trait_id = TypeId::of::<dyn Greeter>();
    let ordinals = HashMap::from([(
        trait_id,
        HashMap::from([
            (TypeId::of::<InnerSpare>(), 0),
            (TypeId::of::<InnerPrimary>(), 1),
        ]),
    )]);
    let generation = Generation::new(
        &[InnerPrimary::descriptor(), InnerSpare::descriptor()],
        vec![
            InnerPrimary::provider().qualifier("shared"),
            InnerSpare::provider().qualifier("shared"),
        ],
        ordinals,
        Reach::EveryChain,
    )
    .await;

    let inner = generation.open_inner(&generation.root, None).await;

    assert_eq!(read_selected(&inner, Some("shared")), Some("inner-spare"));
}
