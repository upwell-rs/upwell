//! Collections and keyed maps read through the providers a layout selected, checked against
//! runtime selection over the chain the scope actually opened in.

use super::*;
use crate::FromContainer;

/// What a collection or keyed dependency on [`Greeter`] resolves to: each provider's qualifier
/// and instance, in the order a collection gathers them, or sorted by qualifier for a map.
type Gathered = Vec<(String, Arc<dyn Greeter>)>;

/// The providers runtime selection gathers for a `cardinality` dependency on [`Greeter`] from
/// `scope`.
async fn gathered_at_runtime(scope: &Arc<ScopeContainer>, cardinality: Cardinality) -> Gathered {
    let mut gathered: Gathered = match cardinality {
        Cardinality::Collection => scope
            .extract_selected::<Vec<Arc<dyn Greeter>>>()
            .await
            .expect("the collection resolves")
            .into_iter()
            .map(|greeter| (greeter.name().to_string(), greeter))
            .collect(),
        Cardinality::Keyed => scope
            .extract_selected::<HashMap<String, Arc<dyn Greeter>>>()
            .await
            .expect("the keyed map resolves")
            .into_iter()
            .collect(),
        Cardinality::One => unreachable!("a single value gathers no providers"),
    };

    if cardinality == Cardinality::Keyed {
        gathered.sort_by(|left, right| left.0.cmp(&right.0));
    }

    gathered
}

/// The names `scope` reads for a `cardinality` dependency on [`Greeter`] through its layout's
/// selected providers, with the qualifier of each for a keyed map, asserting that they are
/// the instances runtime selection gathers. `None` when the read falls back to runtime
/// selection.
async fn read_many(
    scope: &Arc<ScopeContainer>,
    cardinality: Cardinality,
) -> Option<Vec<(String, &'static str)>> {
    let mut read: Gathered = match cardinality {
        Cardinality::Collection => scope
            .built_collection::<Arc<dyn Greeter>>()?
            .into_iter()
            .map(|greeter| (greeter.name().to_string(), greeter))
            .collect(),
        Cardinality::Keyed => scope
            .built_keyed::<Arc<dyn Greeter>>()?
            .into_iter()
            .collect(),
        Cardinality::One => unreachable!("a single value gathers no providers"),
    };

    if cardinality == Cardinality::Keyed {
        read.sort_by(|left, right| left.0.cmp(&right.0));
    }

    let gathered = gathered_at_runtime(scope, cardinality).await;
    let same = read.len() == gathered.len()
        && read.iter().zip(&gathered).all(|(read, gathered)| {
            read.0 == gathered.0
                && std::ptr::addr_eq(Arc::as_ptr(&read.1), Arc::as_ptr(&gathered.1))
        });
    let names = |gathered: &Gathered| -> Vec<(String, &'static str)> {
        gathered
            .iter()
            .map(|(key, greeter)| (key.clone(), greeter.name()))
            .collect()
    };

    assert!(
        same,
        "the table read {:?} but runtime selection gathers {:?}",
        names(&read),
        names(&gathered)
    );

    Some(names(&read))
}

/// The names of the [`Greeter`]s a collection read through the layout holds, in order.
async fn collection(scope: &Arc<ScopeContainer>) -> Option<Vec<&'static str>> {
    let read = read_many(scope, Cardinality::Collection).await?;

    Some(read.into_iter().map(|(_, name)| name).collect())
}

/// The qualifier and name of each [`Greeter`] a keyed read through the layout holds, sorted by
/// qualifier.
async fn keyed(scope: &Arc<ScopeContainer>) -> Option<Vec<(String, &'static str)>> {
    read_many(scope, Cardinality::Keyed).await
}

/// `(qualifier, name)` pairs as [`keyed`] reports them.
fn pairs(pairs: &[(&str, &'static str)]) -> Vec<(String, &'static str)> {
    pairs
        .iter()
        .map(|(qualifier, name)| (qualifier.to_string(), *name))
        .collect()
}

#[tokio::test]
async fn every_provider_on_the_chain_is_read_from_the_layout() {
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
    let extracted = inner
        .extract::<Vec<Arc<dyn Greeter>>>()
        .await
        .expect("the collection extracts");

    assert!(
        <Vec<Arc<dyn Greeter>> as FromContainer>::read_built(&inner).is_some(),
        "extracting a collection reads the layout's providers"
    );
    assert!(
        <HashMap<String, Arc<dyn Greeter>> as FromContainer>::read_built(&inner).is_some(),
        "extracting a keyed map reads the layout's providers"
    );
    assert_eq!(
        collection(&inner).await,
        Some(vec!["root", "inner-primary", "inner-spare"])
    );
    assert_eq!(
        keyed(&inner).await,
        Some(pairs(&[
            ("inner-primary", "inner-primary"),
            ("inner-spare", "inner-spare"),
            ("root", "root"),
        ]))
    );
    assert_eq!(
        extracted
            .iter()
            .map(|greeter| greeter.name())
            .collect::<Vec<_>>(),
        ["root", "inner-primary", "inner-spare"]
    );
}

#[tokio::test]
async fn a_closer_provider_wins_a_shared_qualifier() {
    let generation = Generation::of(
        &[RootGreeter::descriptor(), InnerPrimary::descriptor()],
        vec![
            RootGreeter::provider().qualifier("shared"),
            InnerPrimary::provider().qualifier("shared"),
        ],
        Reach::EveryChain,
    )
    .await;

    let inner = generation.open_inner(&generation.root, None).await;
    let extracted = inner
        .extract::<HashMap<String, Arc<dyn Greeter>>>()
        .await
        .expect("the keyed map extracts");

    assert_eq!(
        keyed(&inner).await,
        Some(pairs(&[("shared", "inner-primary")]))
    );
    assert_eq!(
        extracted.get("shared").map(|greeter| greeter.name()),
        Some("inner-primary")
    );
}

#[tokio::test]
async fn a_parent_off_some_chains_adds_providers_so_the_chain_selects_at_runtime() {
    let generation = Generation::of(
        &[
            RootGreeter::descriptor(),
            OuterGreeter::descriptor(),
            InnerAnchor::descriptor(),
        ],
        vec![
            RootGreeter::provider(),
            OuterGreeter::provider().qualifier("root"),
        ],
        Reach::EveryChain,
    )
    .await;

    let outer = generation.open_outer().await;
    let under_root = generation.open_inner(&generation.root, None).await;
    let under_outer = generation.open_inner(&outer, None).await;
    let extracted = under_outer
        .extract::<Vec<Arc<dyn Greeter>>>()
        .await
        .expect("the collection extracts");

    assert_eq!(collection(&under_root).await, Some(vec!["root"]));
    assert_eq!(keyed(&under_root).await, Some(pairs(&[("root", "root")])));
    assert_eq!(
        collection(&under_outer).await,
        None,
        "the outer provider joins the collection the layout planned"
    );
    assert_eq!(
        keyed(&under_outer).await,
        None,
        "the outer provider wins the qualifier the layout planned for the root one"
    );
    assert_eq!(
        extracted
            .iter()
            .map(|greeter| greeter.name())
            .collect::<Vec<_>>(),
        ["root", "outer"]
    );
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

    assert_eq!(collection(&under_outer).await, Some(vec!["root", "outer"]));
    assert_eq!(
        keyed(&under_outer).await,
        Some(pairs(&[("outer", "outer"), ("root", "root")]))
    );
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

    assert_eq!(collection(&inner).await, None);
    assert_eq!(keyed(&inner).await, None);
    assert_eq!(
        gathered_at_runtime(&inner, Cardinality::Collection)
            .await
            .len(),
        1
    );
}

#[tokio::test]
async fn unbuilt_providers_are_built_by_runtime_selection_then_read() {
    let generation = Generation::of(
        &[InnerPrimary::descriptor(), InnerSpare::descriptor()],
        vec![InnerPrimary::provider(), InnerSpare::provider()],
        Reach::EveryChain,
    )
    .await;

    let inner = generation
        .open_inner(
            &generation.root,
            Some(&EntryPlan::on_demand(&generation.inner)),
        )
        .await;

    assert!(inner.built_collection::<Arc<dyn Greeter>>().is_none());
    assert!(inner.built_keyed::<Arc<dyn Greeter>>().is_none());

    let extracted = inner
        .extract::<Vec<Arc<dyn Greeter>>>()
        .await
        .expect("the collection builds and extracts");

    assert_eq!(extracted.len(), 2);
    assert_eq!(
        collection(&inner).await,
        Some(vec!["inner-primary", "inner-spare"])
    );
}

#[tokio::test]
async fn a_transient_provider_keeps_runtime_selection() {
    let generation = Generation::of(
        &[
            RootGreeter::descriptor(),
            FreshGreeter::descriptor(),
            InnerAnchor::descriptor(),
        ],
        vec![RootGreeter::provider(), FreshGreeter::provider()],
        Reach::EveryChain,
    )
    .await;

    let inner = generation.open_inner(&generation.root, None).await;
    let extracted = inner
        .extract::<Vec<Arc<dyn Greeter>>>()
        .await
        .expect("the collection extracts");

    for cardinality in [Cardinality::Collection, Cardinality::Keyed] {
        assert!(
            generation
                .inner
                .selected_many(TypeId::of::<dyn Greeter>(), cardinality)
                .is_none(),
            "a transient provider is built per resolution, so it is never planned"
        );
    }

    assert_eq!(
        extracted
            .iter()
            .map(|greeter| greeter.name())
            .collect::<Vec<_>>(),
        ["root", "fresh"]
    );
}

#[tokio::test]
async fn no_visible_provider_reads_an_empty_collection() {
    let generation = Generation::of(
        &[OuterGreeter::descriptor(), InnerAnchor::descriptor()],
        vec![OuterGreeter::provider()],
        Reach::EveryChain,
    )
    .await;

    let inner = generation.open_inner(&generation.root, None).await;

    assert_eq!(collection(&inner).await, Some(Vec::new()));
    assert_eq!(keyed(&inner).await, Some(Vec::new()));
}
