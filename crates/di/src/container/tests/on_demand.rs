//! Entry plans and the slots an opening leaves out, built on demand.

use super::*;
use crate::test_support::{component, test_component};
use crate::{FromContainer, dispatch_factory, factory_dependencies};

/// Builds counted so far, per component.
static LEAVES: AtomicUsize = AtomicUsize::new(0);
static BRANCHES: AtomicUsize = AtomicUsize::new(0);
static OTHERS: AtomicUsize = AtomicUsize::new(0);

/// A component without dependencies.
struct Leaf;

/// A component depending on [`Leaf`] in the same scope.
struct Branch {
    leaf: Arc<Leaf>,
}

/// A component no entry targets.
struct Other;

test_component!(Leaf, "leaf");
test_component!(Branch, "branch");
test_component!(Other, "other");

async fn leaf() -> Leaf {
    LEAVES.fetch_add(1, Ordering::SeqCst);

    Leaf
}

async fn branch(leaf: Arc<Leaf>) -> Branch {
    BRANCHES.fetch_add(1, Ordering::SeqCst);

    Branch { leaf }
}

async fn other() -> Other {
    OTHERS.fetch_add(1, Ordering::SeqCst);

    Other
}

/// Declares a `TestScope` component descriptor function `$name` built by factory `$factory`.
macro_rules! scoped {
    ($name:ident, $ty:ty, $id:literal, $factory:ident) => {
        fn $name() -> ComponentDescriptor {
            fn factories() -> &'static [ComponentFactoryDescriptor] {
                const {
                    &[ComponentFactoryDescriptor {
                        id: $id,
                        construct: |cx| dispatch_factory($factory, cx),
                        construct_ready: None,
                        dependencies: || factory_dependencies($factory),
                        role: ComponentFactoryRole::Explicit,
                    }]
                }
            }

            let mut descriptor = component::<$ty>($id, factories);

            descriptor.scope = TestScope::KEY;
            descriptor
        }
    };
}

scoped!(leaf_descriptor, Leaf, "leaf", leaf);
scoped!(branch_descriptor, Branch, "branch", branch);
scoped!(other_descriptor, Other, "other", other);

fn layout() -> Arc<crate::SlotLayout> {
    Arc::new(plan(&[
        leaf_descriptor(),
        branch_descriptor(),
        other_descriptor(),
    ]))
}

async fn open(
    layout: &Arc<crate::SlotLayout>,
    plan: Option<&crate::EntryPlan>,
) -> Arc<ScopeContainer> {
    ScopeContainer::open_planned(
        root().await,
        registry(),
        layout,
        plan,
        Vec::new(),
        Arc::new(()),
    )
    .await
    .expect("the scope opens")
}

fn builds() -> [usize; 3] {
    [&LEAVES, &BRANCHES, &OTHERS].map(|count| count.load(Ordering::SeqCst))
}

#[tokio::test]
async fn an_entry_builds_its_targets_and_their_local_dependencies_only() {
    let layout = layout();
    let plan = layout.plan_entry(&[<Arc<Branch> as FromContainer>::dependency()]);

    let scope = open(&layout, Some(&plan)).await;

    assert_eq!(plan.len(), 2, "the branch and the leaf it depends on");
    assert_eq!(builds(), [1, 1, 0]);
    assert!(
        scope.get::<Other>().is_none(),
        "a synchronous read sees only what the opening built"
    );
}

#[tokio::test]
async fn a_left_out_slot_is_built_once_on_demand() {
    let layout = layout();
    let plan = layout.plan_entry(&[]);
    let scope = open(&layout, Some(&plan)).await;

    let resolutions = (0..8).map(|_| {
        let scope = Arc::clone(&scope);

        tokio::spawn(async move {
            scope
                .resolve::<Arc<Branch>>()
                .await
                .expect("resolution succeeds")
                .expect("the branch is built on demand")
        })
    });
    let branches = futures::future::join_all(resolutions).await;
    let first = branches[0].as_ref().expect("the resolution task finishes");

    assert!(plan.is_empty());
    assert_eq!(builds(), [1, 1, 0], "each slot is built once");
    assert!(branches.iter().all(|branch| {
        Arc::ptr_eq(
            branch.as_ref().expect("the resolution task finishes"),
            first,
        )
    }));
    assert!(Arc::ptr_eq(
        &first.leaf,
        &scope
            .get::<Leaf>()
            .expect("the leaf was built with its branch")
    ));
}

#[tokio::test]
async fn an_extraction_builds_the_slot_it_needs() {
    let layout = layout();
    let plan = layout.plan_entry(&[]);
    let scope = open(&layout, Some(&plan)).await;

    let other = scope
        .extract::<Option<Arc<Other>>>()
        .await
        .expect("extraction succeeds");

    assert!(other.is_some());
    assert_eq!(builds(), [0, 0, 1]);
}

/// A component whose construction awaits a [`Leaf`] it reaches lazily.
struct Eager {
    leaf: Arc<Leaf>,
}

test_component!(Eager, "eager");

fn construct_eager(
    cx: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async move {
        let lazy = <crate::Lazy<Arc<Leaf>> as FromContainer>::from_container(cx).await?;
        let leaf = lazy.get_or_create().await?;

        Ok(boxed_component("Eager", Arc::new(Eager { leaf })))
    })
}

fn eager_descriptor() -> ComponentDescriptor {
    fn factories() -> &'static [ComponentFactoryDescriptor] {
        const {
            &[ComponentFactoryDescriptor {
                id: "eager",
                construct: construct_eager,
                construct_ready: None,
                dependencies: no_dependencies,
                role: ComponentFactoryRole::Explicit,
            }]
        }
    }

    let mut descriptor = component::<Eager>("eager", factories);

    descriptor.scope = TestScope::KEY;
    descriptor
}

#[tokio::test]
async fn a_slot_built_on_demand_can_build_another_while_it_runs() {
    let layout = Arc::new(plan(&[eager_descriptor(), leaf_descriptor()]));
    let plan = layout.plan_entry(&[]);
    let scope = open(&layout, Some(&plan)).await;

    let eager = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        scope.resolve::<Arc<Eager>>(),
    )
    .await
    .expect("building a slot from inside another on-demand build does not deadlock")
    .expect("resolution succeeds")
    .expect("the component is built on demand");

    assert!(Arc::ptr_eq(
        &eager.leaf,
        &scope.get::<Leaf>().expect("the leaf was built on demand")
    ));
}

#[tokio::test]
async fn a_child_builds_the_parent_slots_its_planned_factories_read() {
    let parent_layout = layout();
    let mut child_branch = branch_descriptor();

    child_branch.scope = SameNameScope::KEY;

    let order = [leaf_descriptor(), child_branch];
    let selection =
        crate::registry::ProviderSelectionModel::new(&order, Vec::new(), HashMap::new())
            .expect("the components select no providers");
    let child_layout = Arc::new(
        crate::SlotLayout::plan(
            SameNameScope::KEY,
            &[],
            &[child_branch],
            &HashMap::from([(TestScope::ID, Arc::clone(&parent_layout))]),
            &selection,
            |_, _| true,
        )
        .expect("the child scope plans"),
    );
    let parent = open(&parent_layout, Some(&parent_layout.plan_entry(&[]))).await;

    let child = ScopeContainer::open_planned(
        Arc::clone(&parent),
        registry(),
        &child_layout,
        None,
        Vec::new(),
        Arc::new(()),
    )
    .await
    .expect("the child opens and builds the parent slot its branch reads");

    assert_eq!(builds(), [1, 1, 0]);
    assert!(Arc::ptr_eq(
        &child
            .get::<Branch>()
            .expect("the child built its branch")
            .leaf,
        &parent
            .get::<Leaf>()
            .expect("the parent built its leaf for the child")
    ));
}

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

#[tokio::test]
async fn a_child_builds_only_the_parent_provider_it_reads() {
    let mut prober = prober_descriptor();

    prober.scope = SameNameScope::KEY;

    let parent_order = [chosen_descriptor(), broken_descriptor()];
    let providers = vec![
        ProviderDescriptor::of::<Chosen, dyn Probe>("dyn Probe").primary(),
        ProviderDescriptor::of::<Broken, dyn Probe>("dyn Probe").qualifier("broken"),
    ];
    let all = [chosen_descriptor(), broken_descriptor(), prober];
    let selection = crate::registry::ProviderSelectionModel::new(&all, providers, HashMap::new())
        .expect("the providers name registered components");
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
    let parent = open(&parent_layout, Some(&parent_layout.plan_entry(&[]))).await;

    let child = ScopeContainer::open_planned(
        Arc::clone(&parent),
        registry(),
        &child_layout,
        None,
        Vec::new(),
        Arc::new(()),
    )
    .await
    .expect("the child builds only the provider it reads, not the failing one");

    assert!(child.get::<Prober>().is_some());
    assert!(parent.get::<Chosen>().is_some());
    assert!(
        parent.get::<Broken>().is_none(),
        "the unselected provider stays unbuilt"
    );
}
