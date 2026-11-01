//! Extraction reading a built component directly, and selecting providers otherwise.

use super::*;

/// A trait two components provide.
trait Greeter: Send + Sync {}

/// The primary [`Greeter`].
struct Primary;

/// A [`Greeter`] runtime selection does not pick.
struct Spare;

impl Greeter for Primary {}
impl Greeter for Spare {}

test_component!(Primary, "primary", provides dyn Greeter);
test_component!(Spare, "spare", provides dyn Greeter);

async fn primary() -> Primary {
    Primary
}

async fn spare() -> Spare {
    Spare
}

scoped!(primary_descriptor, Primary, "primary", primary);
scoped!(spare_descriptor, Spare, "spare", spare);

/// A scope holding both greeters whose opening built only the [`Spare`] one.
async fn spare_built() -> Arc<ScopeContainer> {
    let order = [primary_descriptor(), spare_descriptor()];
    let providers = vec![
        ProviderDescriptor::of::<Primary, dyn Greeter>("dyn Greeter").primary(),
        ProviderDescriptor::of::<Spare, dyn Greeter>("dyn Greeter"),
    ];
    let selection =
        crate::registry::ProviderSelectionModel::new(&order, providers.clone(), HashMap::new())
            .expect("the providers name registered components");
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
        ScopeRegistry::new(HashMap::new(), components, providers, HashMap::new())
            .expect("the scope registry validates"),
    );
    let plan = entry_plan(&layout, &[<Arc<Spare> as FromContainer>::dependency()]);

    open_under(root().await, registry, &layout, Some(&plan))
        .await
        .expect("the scope opens")
}

#[tokio::test]
async fn extracting_a_built_component_reads_its_instance() {
    let scope = spare_built().await;

    let spare = scope
        .extract::<Arc<Spare>>()
        .await
        .expect("the built component extracts");
    let optional = scope
        .extract::<Option<Arc<Spare>>>()
        .await
        .expect("the built component extracts as an option");

    assert!(Arc::ptr_eq(
        &spare,
        &scope.get::<Spare>().expect("the opening built the spare")
    ));
    assert!(optional.is_some_and(|optional| Arc::ptr_eq(&optional, &spare)));
}

#[tokio::test]
async fn extracting_a_trait_selects_its_provider_over_a_built_one() {
    let scope = spare_built().await;

    let greeter = scope
        .extract::<Arc<dyn Greeter>>()
        .await
        .expect("the selected provider builds on demand");
    let primary = scope
        .get::<Primary>()
        .expect("extracting the trait built its primary provider");

    assert!(std::ptr::addr_eq(
        Arc::as_ptr(&greeter),
        Arc::as_ptr(&primary)
    ));
}
