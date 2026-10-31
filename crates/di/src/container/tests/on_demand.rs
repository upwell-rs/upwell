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
