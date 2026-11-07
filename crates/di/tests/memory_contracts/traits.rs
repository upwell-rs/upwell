//! Extracting trait targets from a scope opened from its prepared slot layout, which reads the
//! providers the layout selected instead of selecting them per call.

use std::collections::HashMap;
use std::sync::Arc;

use futures::executor::block_on;
use upwell_core::{ResolverSet, Scope, ScopeId, Singleton, TypeDescriptor};
use upwell_di::{
    BoxedComponent, Component, ComponentDescriptor, ComponentRegistry, EntryPlan, FromContainer,
    Injectable, ProviderDescriptor, ProviderOf, ScopeContainer, ScopeRegistry, SlotLayout,
    UnplannedConstruction,
};

use crate::alloc::Delta;
use crate::measure;

/// The trait every fixture component provides.
trait Greeter: Send + Sync {
    fn name(&self) -> &'static str;
}

/// The scope the fixture opens from its layout, under the root.
struct Call;

impl Scope for Call {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/memory-call");
    const RANK: u8 = 1;
    const NAME: &'static str = "Call";
}

/// Declares a seeded [`Greeter`] component `$ty` named `$id`.
macro_rules! greeter {
    ($ty:ident, $id:literal) => {
        #[doc = concat!("The `", $id, "` [`Greeter`].")]
        struct $ty;

        impl Greeter for $ty {
            fn name(&self) -> &'static str {
                $id
            }
        }

        impl Component for $ty {
            type Handle = Arc<Self>;

            const ID: &'static str = $id;
            const NAME: &'static str = stringify!($ty);

            fn into_handle(self) -> Arc<Self> {
                Arc::new(self)
            }
        }

        impl ProviderOf<dyn Greeter> for $ty {
            fn upcast(this: Arc<Self>) -> Arc<dyn Greeter> {
                this
            }
        }

        impl $ty {
            fn descriptor(scope: upwell_core::ScopeKey) -> ComponentDescriptor {
                ComponentDescriptor::manual(
                    $id,
                    stringify!($ty),
                    TypeDescriptor::of::<$ty>(stringify!($ty)),
                    scope,
                )
            }

            /// The instance as a scope below the root stores what its factories build.
            fn seed() -> BoxedComponent {
                BoxedComponent::stored(
                    TypeDescriptor::of::<$ty>(stringify!($ty)),
                    Injectable::into_fixed($ty.into_handle()),
                )
            }

            fn provider() -> ProviderDescriptor {
                ProviderDescriptor::of::<$ty, dyn Greeter>("dyn Greeter")
            }
        }
    };
}

greeter!(RootGreeter, "root");
greeter!(CallPrimary, "call-primary");
greeter!(CallSpare, "call-spare");

/// A [`Call`] scope opened from its layout, and the plan of an entry into it injecting
/// `Arc<dyn Greeter>`.
struct OpenCall {
    scope: Arc<ScopeContainer>,
    plan: EntryPlan,
}

/// Opens a [`Call`] scope from its layout under a root holding [`RootGreeter`], with
/// [`CallPrimary`] (the primary provider) and [`CallSpare`] seeded into it.
///
/// Providers are listed in the order the component registry orders them when nothing else
/// does, by type name, so the scope registry, which keeps the listed order, orders them alike.
async fn open_call() -> OpenCall {
    let root = [RootGreeter::descriptor(Singleton::KEY)];
    let call = [
        CallPrimary::descriptor(Call::KEY),
        CallSpare::descriptor(Call::KEY),
    ];
    let providers = vec![
        CallPrimary::provider().primary(),
        CallSpare::provider(),
        RootGreeter::provider(),
    ];
    let components: Vec<ComponentDescriptor> = root.iter().chain(&call).copied().collect();
    let selection = ComponentRegistry {
        components: components.clone(),
        providers: providers.clone(),
    }
    .provider_selection_model(&components)
    .expect("the fixture's providers name its components");
    let can_reach = |consumer, dependency| consumer == dependency || dependency == Singleton::ID;
    let layout = Arc::new(
        SlotLayout::plan(
            Call::KEY,
            &call,
            &[],
            &HashMap::new(),
            &selection,
            can_reach,
        )
        .expect("the call scope plans"),
    );
    let plan = layout.plan_entry(
        &[<Arc<dyn Greeter>>::dependency()],
        &selection,
        can_reach,
        UnplannedConstruction::Allow,
    );
    let stored = components
        .iter()
        .map(|component| (component.ty.type_id, *component))
        .collect();
    let registry = Arc::new(
        ScopeRegistry::new(HashMap::new(), stored, providers, HashMap::new())
            .expect("the scope registry validates"),
    );
    let root = ScopeContainer::build_root(
        &root,
        vec![RootGreeter::seed()],
        ResolverSet::new(),
        Arc::clone(&registry),
    )
    .await
    .expect("the root builds");

    let scope = ScopeContainer::open_planned(
        root,
        registry,
        &layout,
        Some(&plan),
        vec![CallPrimary::seed(), CallSpare::seed()],
        Arc::new(()),
    )
    .await
    .expect("the call scope opens");

    OpenCall { scope, plan }
}

/// What extracting `H` from `scope` `CALLS` times allocates, through the parameters of `plan`
/// when one is given, after checking with `check` that it extracts what the contract is about,
/// so a contract over a failing extraction cannot pass.
fn extract_repeatedly<H: FromContainer>(
    scope: &Arc<ScopeContainer>,
    plan: Option<&EntryPlan>,
    check: impl Fn(&H) -> bool,
) -> Delta {
    const CALLS: usize = 1_000;

    let extract =
        || block_on(scope.extract_planned::<H>(plan)).expect("the fixture extracts its targets");

    let first = extract();

    assert!(
        check(&first),
        "the fixture extracted something other than the contract expects"
    );

    let (extracted, delta) = measure(|| (0..CALLS).filter(|_| check(&extract())).count());

    assert_eq!(
        extracted, CALLS,
        "extraction regressed during the measurement"
    );
    assert_eq!(
        delta.net_live, 0,
        "extraction leaked {} bytes",
        delta.net_live
    );

    delta
}

#[test]
fn extracting_a_built_trait_target_allocates_nothing() {
    let call = block_on(open_call());
    let is_primary = |greeter: &Arc<dyn Greeter>| greeter.name() == "call-primary";

    assert!(
        <Arc<dyn Greeter>>::read_planned(&call.scope, &call.plan).is_some(),
        "the entry's plan pre-resolved its trait target"
    );

    let single = extract_repeatedly::<Arc<dyn Greeter>>(&call.scope, None, is_primary);
    let optional = extract_repeatedly::<Option<Arc<dyn Greeter>>>(&call.scope, None, |greeter| {
        greeter.as_ref().is_some_and(is_primary)
    });
    let planned = extract_repeatedly::<Arc<dyn Greeter>>(&call.scope, Some(&call.plan), is_primary);

    assert_eq!(
        single.allocations, 0,
        "extracting a built `Arc<dyn Trait>` allocated"
    );
    assert_eq!(
        optional.allocations, 0,
        "extracting a built `Option<Arc<dyn Trait>>` allocated"
    );
    assert_eq!(
        planned.allocations, 0,
        "extracting a planned `Arc<dyn Trait>` parameter allocated"
    );
}

#[test]
fn extracting_built_providers_allocates_only_the_returned_value() {
    const CALLS: u64 = 1_000;
    const PROVIDERS: u64 = 3;

    let call = block_on(open_call());

    let collection = extract_repeatedly::<Vec<Arc<dyn Greeter>>>(&call.scope, None, |greeters| {
        greeters
            .iter()
            .map(|greeter| greeter.name())
            .eq(["call-primary", "call-spare", "root"])
    });
    let keyed =
        extract_repeatedly::<HashMap<String, Arc<dyn Greeter>>>(&call.scope, None, |greeters| {
            greeters.len() as u64 == PROVIDERS
        });

    assert_eq!(
        collection.allocations, CALLS,
        "a collection of built providers allocated more than its `Vec`"
    );
    assert_eq!(
        keyed.allocations,
        CALLS * (1 + PROVIDERS),
        "a keyed map of built providers allocated more than its table and keys"
    );
}
