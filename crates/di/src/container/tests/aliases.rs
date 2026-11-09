//! Provider aliases of a scope opened from a layout are erased only when read.

use futures::executor::block_on;

use super::*;
use crate::test_support::{component, measure, test_component};
use crate::{dispatch_factory, erase_provider, factory_dependencies};

thread_local! {
    /// Provider aliases erased so far on this thread. Each test opens and reads its scopes
    /// on its own thread (`block_on`, or a current-thread runtime), so tests running in
    /// parallel in one process never see each other's counts.
    static ERASED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A trait two scoped components provide.
trait Named: Send + Sync {
    fn name(&self) -> &'static str;
}

/// The primary [`Named`].
struct First;

/// The [`Named`] qualified `second`.
struct Second;

/// A component reading the primary [`Named`] through a planned edge.
struct Reader {
    named: Arc<dyn Named>,
}

impl Named for First {
    fn name(&self) -> &'static str {
        "first"
    }
}

impl Named for Second {
    fn name(&self) -> &'static str {
        "second"
    }
}

test_component!(First, "first", provides dyn Named);
test_component!(Second, "second", provides dyn Named);
test_component!(Reader, "reader");

async fn reader(named: Arc<dyn Named>) -> Reader {
    Reader { named }
}

/// Declares descriptor function `$name` of a `TestScope` component built without a future.
macro_rules! ready {
    ($name:ident, $ty:ident, $id:literal) => {
        fn $name() -> ComponentDescriptor {
            fn construct(
                _: &mut ComponentConstructionContext,
            ) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + '_>> {
                unreachable!("the ready path answers")
            }

            fn factories() -> &'static [ComponentFactoryDescriptor] {
                const {
                    &[ComponentFactoryDescriptor {
                        id: $id,
                        construct,
                        construct_ready: Some(|cx| {
                            Ok(Some(BoxedComponent::stored(
                                TypeDescriptor::of::<$ty>($id),
                                cx.stored(Arc::new($ty)),
                            )))
                        }),
                        dependencies: no_dependencies,
                        role: ComponentFactoryRole::Explicit,
                        scopes: &[],
                    }]
                }
            }

            scoped(component::<$ty>($id, factories))
        }
    };
}

ready!(first, First, "first");
ready!(second, Second, "second");

fn reader_descriptor() -> ComponentDescriptor {
    fn factories() -> &'static [ComponentFactoryDescriptor] {
        const {
            &[ComponentFactoryDescriptor {
                id: "reader",
                construct: |cx| dispatch_factory(reader, cx),
                construct_ready: None,
                dependencies: || factory_dependencies(reader),
                role: ComponentFactoryRole::Explicit,
                scopes: &[],
            }]
        }
    }

    scoped(component::<Reader>("reader", factories))
}

fn scoped(mut descriptor: ComponentDescriptor) -> ComponentDescriptor {
    descriptor.scope = TestScope::KEY;

    descriptor
}

fn counted<T>(concrete: &BoxedComponent) -> BoxedComponent
where
    T: crate::Component + crate::ProviderOf<dyn Named>,
    T::Handle: Injectable<Stored = crate::Live<T>>,
{
    ERASED.set(ERASED.get() + 1);

    erase_provider::<T, dyn Named>(concrete)
}

fn providers() -> Vec<ProviderDescriptor> {
    vec![
        ProviderDescriptor {
            erase: counted::<First>,
            ..ProviderDescriptor::of::<First, dyn Named>("dyn Named").primary()
        },
        ProviderDescriptor {
            erase: counted::<Second>,
            ..ProviderDescriptor::of::<Second, dyn Named>("dyn Named").qualifier("second")
        },
    ]
}

/// A planned scope holding `order` with `providers`, and the registry it opens with.
fn planned(
    order: &[ComponentDescriptor],
    providers: Vec<ProviderDescriptor>,
) -> (Arc<crate::SlotLayout>, Arc<ScopeRegistry>) {
    let selection = Arc::new(
        crate::registry::ProviderSelectionModel::new(order, providers, HashMap::new())
            .expect("the providers name registered components"),
    );
    let layout = crate::SlotLayout::plan(
        TestScope::KEY,
        &[],
        order,
        &HashMap::new(),
        &selection,
        |_, _| true,
    )
    .expect("the scope plans");
    let components = order
        .iter()
        .map(|component| (component.ty.type_id, *component))
        .collect();
    let registry = ScopeRegistry::from_selection_model(HashMap::new(), components, selection)
        .expect("the registry validates");

    (Arc::new(layout), Arc::new(registry))
}

async fn open(
    parent: Arc<ScopeContainer>,
    layout: &Arc<crate::SlotLayout>,
    registry: &Arc<ScopeRegistry>,
) -> Arc<ScopeContainer> {
    ScopeContainer::open_planned(
        parent,
        Arc::clone(registry),
        layout,
        None,
        Vec::new(),
        Arc::new(()),
    )
    .await
    .expect("the scope opens")
}

#[test]
fn opening_a_scope_erases_no_alias_and_allocates_none() {
    let order = [first(), second()];
    let parent = block_on(root());
    let (aliased, aliased_registry) = planned(&order, providers());
    let (plain, plain_registry) = planned(&order, Vec::new());

    let (_, with_aliases) = measure(|| {
        block_on(open(Arc::clone(&parent), &aliased, &aliased_registry));
    });
    let (_, without_aliases) = measure(|| {
        block_on(open(Arc::clone(&parent), &plain, &plain_registry));
    });

    assert_eq!(ERASED.get(), 0, "nothing read an alias of the opened scope");
    assert_eq!(
        with_aliases.allocations, without_aliases.allocations,
        "a provider nothing reads adds no allocation to opening its scope"
    );
}

#[tokio::test]
async fn a_planned_edge_erases_only_the_alias_it_reads() {
    let order = [first(), second(), reader_descriptor()];
    let (layout, registry) = planned(&order, providers());

    let scope = open(root().await, &layout, &registry).await;
    let reader = scope.get::<Reader>().expect("the scope built the reader");

    assert_eq!(reader.named.name(), "first");
    assert_eq!(ERASED.get(), 1, "only the alias the edge reads was erased");
}

#[tokio::test]
async fn dynamic_lookups_see_every_provider_of_an_opened_scope() {
    let order = [first(), second()];
    let (layout, registry) = planned(&order, providers());

    let scope = open(root().await, &layout, &registry).await;
    let collected = scope
        .extract::<Vec<Arc<dyn Named>>>()
        .await
        .expect("the collection resolves");
    let keyed = scope
        .extract::<HashMap<String, Arc<dyn Named>>>()
        .await
        .expect("the keyed map resolves");
    let qualified = scope
        .resolve_qualified::<Arc<dyn Named>>("second")
        .await
        .expect("the qualified provider resolves")
        .expect("the qualifier selects a provider");
    let names: Vec<_> = collected.iter().map(|named| named.name()).collect();

    assert_eq!(names, ["first", "second"]);
    assert_eq!(keyed.len(), 2, "the map holds every qualifier");
    assert_eq!(qualified.name(), "second");
}

#[test]
fn an_arc_component_costs_one_allocation_for_its_storage() {
    let parent = block_on(root());
    let (one, one_registry) = planned(&[first()], Vec::new());
    let (two, two_registry) = planned(&[first(), second()], Vec::new());

    let (_, with_one) = measure(|| {
        block_on(open(Arc::clone(&parent), &one, &one_registry));
    });
    let (_, with_two) = measure(|| {
        block_on(open(Arc::clone(&parent), &two, &two_registry));
    });

    assert_eq!(
        with_two.allocations,
        with_one.allocations + 1,
        "another instance costs its own `Arc` and nothing around it"
    );
}

#[test]
fn reading_a_provider_alias_allocates_nothing() {
    let order = [first(), second()];
    let (layout, registry) = planned(&order, providers());
    let scope = block_on(async { open(root().await, &layout, &registry).await });

    let (named, delta) =
        measure(|| scope.selected_built::<Arc<dyn Named>>(TypeId::of::<dyn Named>(), None));

    assert_eq!(named.expect("the primary is built").name(), "first");
    assert_eq!(delta.allocations, 0, "the alias lives only for the read");
}
