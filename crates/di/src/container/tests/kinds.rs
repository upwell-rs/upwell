//! Container kinds: each scope type's generation policy picks how its instances store slots.

use std::sync::atomic::AtomicBool;

use futures::executor::block_on;
use upwell_core::ScopeGenerations;

use super::*;
use crate::layout::{ContainerKind, EntryPlan, SlotLayout};
use crate::test_support::{component, measure};

/// A scope type that spans commits, as a connection does.
struct DurableScope;

impl Scope for DurableScope {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/durable");
    const RANK: u8 = 1;
    const NAME: &'static str = "Durable";
    const GENERATIONS: ScopeGenerations = ScopeGenerations::Durable;
}

const IDS: [&str; 16] = [
    "slot-0", "slot-1", "slot-2", "slot-3", "slot-4", "slot-5", "slot-6", "slot-7", "slot-8",
    "slot-9", "slot-10", "slot-11", "slot-12", "slot-13", "slot-14", "slot-15",
];

/// One of sixteen distinct component types.
struct Slot<const N: usize>;

impl<const N: usize> Component for Slot<N> {
    type Handle = Arc<Self>;

    const ID: &'static str = IDS[N];
    const NAME: &'static str = IDS[N];

    fn into_handle(self) -> Arc<Self> {
        Arc::new(self)
    }
}

/// Whether [`Slot<0>`]'s factory fails.
static SLOT_FAILS: AtomicBool = AtomicBool::new(false);

/// Builds `Slot<N>` on the synchronous path, allocating only its `Arc`.
fn ready<const N: usize>(
    cx: &ComponentConstructionContext,
) -> crate::Result<Option<BoxedComponent>> {
    if N == 0 && SLOT_FAILS.load(Ordering::SeqCst) {
        return Err(Error::MissingComponent(IDS[N]));
    }

    let handle = cx.stored(Arc::new(Slot::<N>));

    Ok(Some(BoxedComponent::stored(
        TypeDescriptor::of::<Slot<N>>(IDS[N]),
        handle,
    )))
}

fn unready(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async { Err(Error::MissingComponent("a ready factory went async")) })
}

fn factories<const N: usize>() -> &'static [ComponentFactoryDescriptor] {
    const {
        &[ComponentFactoryDescriptor {
            id: IDS[N],
            construct: unready,
            construct_ready: Some(ready::<N>),
            dependencies: no_dependencies,
            role: ComponentFactoryRole::Explicit,
        }]
    }
}

fn seedless() -> &'static [ComponentFactoryDescriptor] {
    &[]
}

/// `Slot<N>` in scope `S`, built by a factory, or seeded when `seeded`.
fn slot<S: Scope, const N: usize>(seeded: bool) -> ComponentDescriptor {
    let factories = match seeded {
        true => seedless,
        false => factories::<N>,
    };
    let mut descriptor = component::<Slot<N>>(IDS[N], factories);

    descriptor.scope = S::KEY;

    descriptor
}

/// Declares the per-index tables of the sixteen slot types, so every slot and its seed come
/// from one index list.
macro_rules! per_slot {
    ($($n:literal),* $(,)?) => {
        /// Every slot descriptor of scope `S`, in index order.
        fn all_slots<S: Scope>(seeded: bool) -> Vec<ComponentDescriptor> {
            vec![$(slot::<S, $n>(seeded)),*]
        }

        /// The seed filling each slot, in index order.
        const SEEDS: &[fn() -> BoxedComponent] = &[$(seed::<$n>),*];
    };
}

per_slot!(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15);

/// The seed filling `Slot<N>`.
fn seed<const N: usize>() -> BoxedComponent {
    BoxedComponent::stored(
        TypeDescriptor::of::<Slot<N>>(IDS[N]),
        <Arc<Slot<N>> as Injectable>::into_fixed(Arc::new(Slot::<N>)),
    )
}

/// The first `count` slot descriptors of scope `S`.
fn slots<S: Scope>(count: usize, seeded: bool) -> Vec<ComponentDescriptor> {
    let mut all = all_slots::<S>(seeded);

    all.truncate(count);

    all
}

/// The seeds filling every slot of [`slots`]`(count, true)`.
fn seeds(count: usize) -> Vec<BoxedComponent> {
    SEEDS[..count].iter().map(|seed| seed()).collect()
}

/// The layout of scope `S` holding `components`, seeds or factory-backed alike.
fn layout_of<S: Scope>(components: &[ComponentDescriptor]) -> Arc<SlotLayout> {
    let (seeded, built): (Vec<_>, Vec<_>) = components
        .iter()
        .partition(|descriptor| descriptor.effective_factory().ok().flatten().is_none());
    let selection =
        crate::registry::ProviderSelectionModel::new(&built, Vec::new(), HashMap::new())
            .expect("the components select no providers");

    Arc::new(
        SlotLayout::plan(
            S::KEY,
            &seeded,
            &built,
            &HashMap::new(),
            &selection,
            |_, _| true,
        )
        .expect("the scope plans"),
    )
}

/// Opens `layout` under the root of `parent` with `seeds`, building what `plan` lists.
async fn open_with(
    parent: &Arc<ScopeContainer>,
    layout: &Arc<SlotLayout>,
    plan: Option<&EntryPlan>,
    seeds: Vec<BoxedComponent>,
    lease: &Arc<()>,
) -> Arc<ScopeContainer> {
    ScopeContainer::open_planned(
        Arc::clone(parent),
        parent.registry(),
        layout,
        plan,
        seeds,
        Arc::clone(lease),
    )
    .await
    .expect("the scope opens")
}

fn kind_of(container: &ScopeContainer) -> ContainerKind {
    container
        .store()
        .slots()
        .expect("the scope was opened from a layout")
        .kind()
}

/// The allocations opening scope `S` makes with `count` slots, seeded or built.
fn opening_allocations<S: Scope>(count: usize, seeded: bool) -> u64 {
    let parent = block_on(root());
    let lease = Arc::new(());
    let layout = layout_of::<S>(&slots::<S>(count, seeded));
    let build = || match seeded {
        true => seeds(count),
        false => Vec::new(),
    };

    drop(block_on(open_with(&parent, &layout, None, build(), &lease)));

    let pending = build();
    let (_, delta) = measure(|| drop(block_on(open_with(&parent, &layout, None, pending, &lease))));

    delta.allocations
}

#[tokio::test]
async fn a_scope_type_picks_its_container_kind_from_its_generation_policy() {
    let parent = root().await;
    let lease = Arc::new(());
    let pinned = layout_of::<TestScope>(&slots::<TestScope>(1, false));
    let durable = layout_of::<DurableScope>(&slots::<DurableScope>(1, false));

    let pinned = open_with(&parent, &pinned, None, Vec::new(), &lease).await;
    let durable = open_with(&parent, &durable, None, Vec::new(), &lease).await;

    assert_eq!(TestScope::KEY.generations(), ScopeGenerations::Pinned);
    assert_eq!(
        kind_of(&pinned),
        ContainerKind::Pinned,
        "the default policy pins"
    );
    assert_eq!(kind_of(&durable), ContainerKind::Durable);
    assert!(pinned.get::<Slot<0>>().is_some() && durable.get::<Slot<0>>().is_some());
}

#[test]
fn opening_a_scope_allocates_its_slots_in_one_block() {
    for count in [1, 4, 16] {
        assert_eq!(
            opening_allocations::<TestScope>(count, true),
            2,
            "a pinned scope seeding {count} slots allocates its container and one slot block"
        );
        assert_eq!(
            opening_allocations::<DurableScope>(count, true),
            2,
            "a durable scope seeding {count} slots allocates its container and one cell block"
        );
    }
}

#[test]
fn building_k_planned_components_adds_only_their_own_allocations() {
    for count in [1, 4, 16] {
        let expected = 2 + count as u64;

        assert_eq!(
            opening_allocations::<TestScope>(count, false),
            expected,
            "a pinned scope building {count} components allocates them, its container and one \
             slot block"
        );
        assert_eq!(
            opening_allocations::<DurableScope>(count, false),
            expected,
            "a durable scope building {count} components allocates them, its container and one \
             cell block"
        );
    }
}

/// The allocations building `Slot<0>` on demand makes in an instance of `S` that opened
/// building nothing.
fn on_demand_allocations<S: Scope>() -> u64 {
    let parent = block_on(root());
    let lease = Arc::new(());
    let layout = layout_of::<S>(&slots::<S>(2, false));
    let plan = EntryPlan::on_demand(&layout);
    let warm = block_on(open_with(&parent, &layout, Some(&plan), Vec::new(), &lease));

    block_on(warm.resolve::<Arc<Slot<0>>>()).expect("the slot builds");

    let scope = block_on(open_with(&parent, &layout, Some(&plan), Vec::new(), &lease));
    let (built, delta) = measure(|| block_on(scope.resolve::<Arc<Slot<0>>>()));

    assert!(
        built.expect("the slot builds").is_some(),
        "the on-demand build resolved nothing; the count would be vacuous"
    );

    delta.allocations
}

#[test]
fn a_durable_scope_builds_on_demand_into_the_cells_it_opened_with() {
    assert_eq!(
        on_demand_allocations::<TestScope>(),
        on_demand_allocations::<DurableScope>() + 1,
        "a pinned scope allocates cells for its first on-demand build; a durable one has them"
    );
}

#[tokio::test]
async fn a_failed_on_demand_build_is_not_kept() {
    let parent = root().await;
    let lease = Arc::new(());
    let layout = layout_of::<DurableScope>(&slots::<DurableScope>(1, false));
    let plan = EntryPlan::on_demand(&layout);
    let scope = open_with(&parent, &layout, Some(&plan), Vec::new(), &lease).await;

    SLOT_FAILS.store(true, Ordering::SeqCst);

    let failed = scope.resolve::<Arc<Slot<0>>>().await;

    SLOT_FAILS.store(false, Ordering::SeqCst);

    let retried = scope.resolve::<Arc<Slot<0>>>().await;

    assert!(
        matches!(failed, Err(Error::MissingComponent("slot-0"))),
        "the caller that triggered the build sees its failure"
    );
    assert!(
        retried.expect("the next resolution builds again").is_some(),
        "a failed build is not cached"
    );
}
