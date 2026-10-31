//! Building only the providers runtime selection picks, at opening and on demand.

use super::*;

/// A trait two parent-scope components provide.
trait Probe: Send + Sync {}

/// The primary [`Probe`].
struct Chosen;

/// A [`Probe`] whose factory fails.
struct Broken;

/// A child-scope component reading the selected [`Probe`] from its parent scope.
struct Prober {
    _probe: Arc<dyn Probe>,
}

impl Probe for Chosen {}
impl Probe for Broken {}

test_component!(Chosen, "chosen", provides dyn Probe);
test_component!(Broken, "broken", provides dyn Probe);
test_component!(Prober, "prober");

async fn chosen() -> Chosen {
    Chosen
}

async fn prober(probe: Arc<dyn Probe>) -> Prober {
    Prober { _probe: probe }
}

fn construct_broken(
    _cx: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async { Err(Error::MissingComponent("broken")) })
}

scoped!(chosen_descriptor, Chosen, "chosen", chosen);
scoped!(prober_descriptor, Prober, "prober", prober);

fn broken_descriptor() -> ComponentDescriptor {
    fn factories() -> &'static [ComponentFactoryDescriptor] {
        const {
            &[ComponentFactoryDescriptor {
                id: "broken",
                construct: construct_broken,
                construct_ready: None,
                dependencies: no_dependencies,
                role: ComponentFactoryRole::Explicit,
            }]
        }
    }

    let mut descriptor = component::<Broken>("broken", factories);

    descriptor.scope = TestScope::KEY;

    descriptor
}

/// `Chosen` as the primary [`Probe`] and `Broken` as the one qualified `broken`.
fn probe_providers() -> Vec<ProviderDescriptor> {
    vec![
        ProviderDescriptor::of::<Chosen, dyn Probe>("dyn Probe").primary(),
        ProviderDescriptor::of::<Broken, dyn Probe>("dyn Probe").qualifier("broken"),
    ]
}

/// The selection model of `order` with the [`probe_providers`].
fn select(order: &[ComponentDescriptor]) -> crate::registry::ProviderSelectionModel {
    crate::registry::ProviderSelectionModel::new(order, probe_providers(), HashMap::new())
        .expect("the providers name registered components")
}

#[tokio::test]
async fn a_child_builds_only_the_parent_provider_it_reads() {
    let mut prober = prober_descriptor();

    prober.scope = SameNameScope::KEY;

    let parent_order = [chosen_descriptor(), broken_descriptor()];
    let selection = select(&[chosen_descriptor(), broken_descriptor(), prober]);
    let no_ancestors = HashMap::new();
    let plan_scope = |scope,
                      order: &[ComponentDescriptor],
                      ancestors: &HashMap<ScopeId, Arc<crate::SlotLayout>>| {
        crate::SlotLayout::plan(scope, &[], order, ancestors, &selection, |_, _| true)
            .expect("the scope plans")
    };
    let parent_layout = Arc::new(plan_scope(TestScope::KEY, &parent_order, &no_ancestors));
    let child_layout = Arc::new(plan_scope(
        SameNameScope::KEY,
        &[prober],
        &HashMap::from([(TestScope::ID, Arc::clone(&parent_layout))]),
    ));
    let parent = open(&parent_layout, Some(&entry_plan(&parent_layout, &[]))).await;

    let child = open_under(Arc::clone(&parent), registry(), &child_layout, None)
        .await
        .expect("the child builds only the provider it reads, not the failing one");

    assert!(child.get::<Prober>().is_some());
    assert!(parent.get::<Chosen>().is_some());
    assert!(
        parent.get::<Broken>().is_none(),
        "the unselected provider stays unbuilt"
    );
}

#[tokio::test]
async fn resolving_one_provider_builds_only_the_selected_one() {
    let order = [chosen_descriptor(), broken_descriptor()];
    let selection = select(&order);
    let layout = Arc::new(
        crate::SlotLayout::plan(
            TestScope::KEY,
            &[],
            &order,
            &HashMap::new(),
            &selection,
            |_, _| true,
        )
        .expect("the scope plans"),
    );
    let components = order
        .iter()
        .map(|component| (component.ty.type_id, *component))
        .collect();
    let registry = Arc::new(
        ScopeRegistry::new(
            HashMap::new(),
            components,
            probe_providers(),
            HashMap::new(),
        )
        .expect("the scope registry validates"),
    );
    let plan = layout.plan_entry(
        &[<Arc<dyn Probe> as FromContainer>::dependency()],
        &selection,
        |_, _| true,
    );
    let planned = open_under(root().await, Arc::clone(&registry), &layout, Some(&plan))
        .await
        .expect("opening with the entry plan does not build the failing provider");
    let scope = open_under(
        root().await,
        registry,
        &layout,
        Some(&entry_plan(&layout, &[])),
    )
    .await
    .expect("the scope opens");

    assert!(
        planned.get::<Chosen>().is_some(),
        "the entry plan builds the primary provider"
    );
    assert!(planned.get::<Broken>().is_none());

    let probe = scope
        .resolve::<Arc<dyn Probe>>()
        .await
        .expect("resolving the primary provider does not build the failing one");

    assert_eq!(
        plan.len(),
        1,
        "an entry injecting the trait plans only its primary provider"
    );
    assert!(probe.is_some());
    assert!(scope.get::<Chosen>().is_some());
    assert!(
        scope.get::<Broken>().is_none(),
        "the unselected provider stays unbuilt"
    );
}
