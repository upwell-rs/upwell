//! Tests for [`ScopeContainer::live_cell`] and the [`LiveCell`] republication seam.

use std::collections::HashMap;
use std::sync::Arc;

use upwell_core::{ResolverSet, ScopeId, Singleton, StaticScope};

use crate::container::ScopeContainer;
use crate::test_support::test_component;
use crate::{
    BoxedComponent, Component, ComponentDescriptor, Dep, EffectiveTarget, Error, Injectable,
    LiveCell, ProviderDescriptor, ScopeRegistry,
};

trait Greeter: Send + Sync {
    fn greeting(&self) -> &'static str;
}

struct EnglishGreeter;

impl Greeter for EnglishGreeter {
    fn greeting(&self) -> &'static str {
        "hello"
    }
}

test_component!(EnglishGreeter, "live-english", provides dyn Greeter);

struct SwedishGreeter;

impl Greeter for SwedishGreeter {
    fn greeting(&self) -> &'static str {
        "hej"
    }
}

test_component!(SwedishGreeter, "live-swedish", provides dyn Greeter);

const ENGLISH: ComponentDescriptor = ComponentDescriptor::of::<EnglishGreeter>();
const SWEDISH: ComponentDescriptor = ComponentDescriptor::of::<SwedishGreeter>();
const ENGLISH_PROVIDER: ProviderDescriptor =
    ProviderDescriptor::of::<EnglishGreeter, dyn Greeter>("dyn Greeter");

fn seed<T: Component>(descriptor: &ComponentDescriptor, component: T) -> BoxedComponent {
    BoxedComponent {
        ty: descriptor.ty,
        value: Box::new(Injectable::into_stored(component.into_handle())),
    }
}

/// Builds a root seeded with one English and one Swedish greeter, mapping only the
/// English one onto `dyn Greeter`.
async fn greeter_root() -> Arc<ScopeContainer> {
    let descriptors = [ENGLISH, SWEDISH];
    let registry = ScopeRegistry::new(
        HashMap::new(),
        descriptors
            .iter()
            .map(|descriptor| (descriptor.ty.type_id, *descriptor))
            .collect(),
        vec![ENGLISH_PROVIDER],
        HashMap::new(),
    )
    .expect("greeter registry validates");

    ScopeContainer::build_root(
        &descriptors,
        vec![
            seed(&ENGLISH, EnglishGreeter),
            seed(&SWEDISH, SwedishGreeter),
        ],
        ResolverSet::new(),
        Arc::new(registry),
    )
    .await
    .expect("greeter root builds")
}

fn component_target(descriptor: &ComponentDescriptor) -> EffectiveTarget {
    EffectiveTarget::Component {
        component: descriptor.id,
        scope: Singleton::ID,
    }
}

fn provider_target() -> EffectiveTarget {
    EffectiveTarget::Provider {
        mapping: ENGLISH_PROVIDER.mapping_id(&ENGLISH),
        scope: Singleton::ID,
    }
}

#[tokio::test]
async fn component_target_yields_the_cell_dep_consumers_share() {
    let root = greeter_root().await;

    let cell = root
        .live_cell(component_target(&ENGLISH))
        .expect("a singleton component slot stores a live cell");
    let dep = root
        .resolve::<Dep<EnglishGreeter>>()
        .await
        .expect("live resolution succeeds")
        .expect("the English greeter resolves as a live handle");

    assert!(cell.same_cell(&LiveCell::of(&dep)));
}

#[tokio::test]
async fn provider_target_yields_the_trait_slot_dep_consumers_share() {
    let root = greeter_root().await;

    let cell = root
        .live_cell(provider_target())
        .expect("a singleton provider mapping stores a live trait cell");
    let dep = root
        .resolve::<Dep<dyn Greeter>>()
        .await
        .expect("live resolution succeeds")
        .expect("the mapped greeter resolves as a live trait handle");
    let concrete = root
        .live_cell(component_target(&ENGLISH))
        .expect("the concrete slot stores its own live cell");

    assert!(cell.same_cell(&LiveCell::of(&dep)));
    assert!(
        !cell.same_cell(&concrete),
        "the erased trait slot is a separate cell from the concrete slot"
    );
}

#[tokio::test]
async fn non_singleton_and_unmapped_targets_store_no_live_cell() {
    struct OtherScope;

    impl StaticScope for OtherScope {
        const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/live-other");
        const RANK: u8 = 1;
        const NAME: &'static str = "LiveOther";
    }

    let root = greeter_root().await;
    let swedish_mapping =
        ProviderDescriptor::of::<SwedishGreeter, dyn Greeter>("dyn Greeter").mapping_id(&SWEDISH);

    let scoped = root.live_cell(EffectiveTarget::Component {
        component: ENGLISH.id,
        scope: OtherScope::ID,
    });
    let unmapped = root.live_cell(EffectiveTarget::Provider {
        mapping: swedish_mapping,
        scope: Singleton::ID,
    });

    assert!(matches!(
        scoped,
        Err(Error::LiveCellUnavailable {
            component: "live-english"
        })
    ));
    assert!(matches!(
        unmapped,
        Err(Error::LiveCellUnavailable {
            component: "live-swedish"
        })
    ));
}

#[tokio::test]
async fn a_rebind_publishes_only_when_committed() {
    let root = greeter_root().await;
    let held = root
        .live_cell(provider_target())
        .expect("the trait slot stores a live cell");
    let consumer = root
        .resolve::<Dep<dyn Greeter>>()
        .await
        .expect("live resolution succeeds")
        .expect("the consumer's live trait handle resolves");
    let snapshot = consumer.snapshot();
    let swedish: Dep<dyn Greeter> = Dep::new(Arc::new(SwedishGreeter));
    let source = LiveCell::of(&swedish);

    let dropped = held
        .rebind_to(&source)
        .expect("both cells hold dyn Greeter");

    drop(dropped);

    assert_eq!(
        consumer.get().greeting(),
        "hello",
        "a dropped rebind leaves the held cell untouched"
    );

    held.rebind_to(&source)
        .expect("both cells hold dyn Greeter")
        .commit();

    assert_eq!(consumer.get().greeting(), "hej");
    assert_eq!(
        snapshot.greeting(),
        "hello",
        "an earlier snapshot keeps the previous instance"
    );
}

#[tokio::test]
async fn a_rebind_between_different_types_is_rejected() {
    let root = greeter_root().await;
    let trait_cell = root
        .live_cell(provider_target())
        .expect("the trait slot stores a live cell");
    let concrete_cell = root
        .live_cell(component_target(&SWEDISH))
        .expect("the concrete slot stores a live cell");

    assert!(trait_cell.rebind_to(&concrete_cell).is_none());
}
