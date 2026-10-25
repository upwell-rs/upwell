//! Scope-local provider selection: a child scope's own provider takes precedence
//! over a parent's globally primary provider, and the build order must reflect it.

use std::{
    any::TypeId,
    collections::{HashMap, HashSet},
    sync::Arc,
};

use upwell::{
    ComponentDescriptor, ComponentRegistry, Descriptor, PROVIDERS, ResolverSet, Scope,
    ScopeContainer, ScopeId, ScopeRegistry, component, injectable, topological_sort,
};

/// A throwaway child scope for scope-local provider selection tests.
struct ChildScope;

impl Scope for ChildScope {
    const ID: ScopeId = upwell::namespaced_id!(ScopeId, "test/provider-child");
    const RANK: u8 = 1;
    const NAME: &'static str = "Child";
}

fn can_reach(consumer: ScopeId, dependency: ScopeId) -> bool {
    consumer == dependency || (consumer == ChildScope::ID && dependency == upwell::Singleton::ID)
}

/// A trait with providers in two scopes, exercising scope-local selection.
#[injectable]
trait ScopedChoice: Send + Sync {
    fn name(&self) -> &'static str;
}

/// The parent-scope provider, globally primary.
#[component(provide = dyn ScopedChoice, primary)]
struct ParentPrimaryChoice;

impl ScopedChoice for ParentPrimaryChoice {
    fn name(&self) -> &'static str {
        "parent"
    }
}

/// The child scope's own sole provider: it takes precedence over the parent's
/// primary for child-scope consumers at runtime.
#[component(scope = ChildScope, provide = dyn ScopedChoice)]
struct ChildLocalChoice;

impl ScopedChoice for ChildLocalChoice {
    fn name(&self) -> &'static str {
        "child"
    }
}

/// A child-scope consumer of the trait's single provider.
#[component(scope = ChildScope)]
struct ScopedChoiceConsumer {
    chosen: Arc<dyn ScopedChoice>,
}

/// A trait with a primary parent provider and two non-primary child providers:
/// the child-local set is ambiguous, so runtime falls back to the parent — but
/// only deterministically once every local provider is registered.
#[injectable]
trait AmbiguousChoice: Send + Sync {
    fn name(&self) -> &'static str;
}

/// The parent-scope primary the ambiguous local set falls back to.
#[component(provide = dyn AmbiguousChoice, primary)]
struct AmbiguousParentPrimary;

impl AmbiguousChoice for AmbiguousParentPrimary {
    fn name(&self) -> &'static str {
        "parent"
    }
}

/// The first non-primary child provider.
#[component(scope = ChildScope, provide = dyn AmbiguousChoice)]
struct FirstLocalChoice;

impl AmbiguousChoice for FirstLocalChoice {
    fn name(&self) -> &'static str {
        "first"
    }
}

/// The second non-primary child provider, making the local set ambiguous.
#[component(scope = ChildScope, provide = dyn AmbiguousChoice)]
struct SecondLocalChoice;

impl AmbiguousChoice for SecondLocalChoice {
    fn name(&self) -> &'static str {
        "second"
    }
}

/// A child consumer facing the ambiguous local set.
#[component(scope = ChildScope)]
struct AmbiguousLocalConsumer {
    chosen: Arc<dyn AmbiguousChoice>,
}

#[tokio::test]
async fn child_scope_consumer_waits_for_its_scope_local_provider() {
    let parent = <ParentPrimaryChoice as Descriptor<ComponentDescriptor>>::DESCRIPTOR;
    let local = <ChildLocalChoice as Descriptor<ComponentDescriptor>>::DESCRIPTOR;
    let consumer = <ScopedChoiceConsumer as Descriptor<ComponentDescriptor>>::DESCRIPTOR;
    let components = [parent, local, consumer];
    let providers: Vec<_> = PROVIDERS
        .iter()
        .filter(|provider| provider.trait_ty.type_id == TypeId::of::<dyn ScopedChoice>())
        .copied()
        .collect();
    let selection = Arc::new(
        ComponentRegistry {
            components: components.to_vec(),
            providers: providers.clone(),
        }
        .provider_selection_model(&components)
        .expect("provider selection model validates"),
    );
    let registry = Arc::new(
        ScopeRegistry::from_selection_model(
            HashMap::new(),
            components
                .iter()
                .map(|component| (component.ty.type_id, *component))
                .collect::<HashMap<TypeId, ComponentDescriptor>>(),
            Arc::clone(&selection),
        )
        .expect("scope registry validates"),
    );

    let root =
        ScopeContainer::build_root(&[parent], Vec::new(), ResolverSet::new(), registry.clone())
            .await
            .expect("root builds");
    let prebuilt: HashSet<TypeId> = [parent.ty.type_id].into_iter().collect();

    // The consumer is listed first on purpose: the sort must move it after the
    // child-scope provider it actually captures, not treat the globally primary
    // (already-built) parent provider as its wait set.
    let child_components = [consumer, local];
    let order = topological_sort(&child_components, &prebuilt, &selection, can_reach)
        .expect("child scope sorts");
    let names: Vec<_> = order.iter().map(|component| component.name).collect();

    assert_eq!(names, ["ChildLocalChoice", "ScopedChoiceConsumer"]);

    let order: Vec<ComponentDescriptor> = order.into_iter().copied().collect();
    let child = ScopeContainer::open_child(ChildScope::KEY, root, registry, &order, Vec::new())
        .await
        .expect("child scope builds");

    assert_eq!(
        child
            .get::<ScopedChoiceConsumer>()
            .expect("consumer built")
            .chosen
            .name(),
        "child",
        "the consumer captures the scope-local provider, not the parent primary"
    );
}

#[tokio::test]
async fn ambiguous_local_set_waits_for_all_locals_before_parent_fallback() {
    let parent = <AmbiguousParentPrimary as Descriptor<ComponentDescriptor>>::DESCRIPTOR;
    let first = <FirstLocalChoice as Descriptor<ComponentDescriptor>>::DESCRIPTOR;
    let second = <SecondLocalChoice as Descriptor<ComponentDescriptor>>::DESCRIPTOR;
    let consumer = <AmbiguousLocalConsumer as Descriptor<ComponentDescriptor>>::DESCRIPTOR;
    let components = [parent, first, second, consumer];
    let providers: Vec<_> = PROVIDERS
        .iter()
        .filter(|provider| provider.trait_ty.type_id == TypeId::of::<dyn AmbiguousChoice>())
        .copied()
        .collect();
    let selection = Arc::new(
        ComponentRegistry {
            components: components.to_vec(),
            providers: providers.clone(),
        }
        .provider_selection_model(&components)
        .expect("provider selection model validates"),
    );
    let registry = Arc::new(
        ScopeRegistry::from_selection_model(
            HashMap::new(),
            components
                .iter()
                .map(|component| (component.ty.type_id, *component))
                .collect::<HashMap<TypeId, ComponentDescriptor>>(),
            Arc::clone(&selection),
        )
        .expect("scope registry validates"),
    );

    let root =
        ScopeContainer::build_root(&[parent], Vec::new(), ResolverSet::new(), registry.clone())
            .await
            .expect("root builds");
    let prebuilt: HashSet<TypeId> = [parent.ty.type_id].into_iter().collect();

    // The consumer is listed first: it must build after BOTH local providers, or
    // runtime would see a partially registered local set as temporarily sole and
    // capture it instead of deterministically falling back to the parent primary.
    let child_components = [consumer, first, second];
    let order = topological_sort(&child_components, &prebuilt, &selection, can_reach)
        .expect("child scope sorts");
    let names: Vec<_> = order.iter().map(|component| component.name).collect();

    assert_eq!(names.last(), Some(&"AmbiguousLocalConsumer"));
    assert_eq!(names.len(), 3);

    let order: Vec<ComponentDescriptor> = order.into_iter().copied().collect();
    let child = ScopeContainer::open_child(ChildScope::KEY, root, registry, &order, Vec::new())
        .await
        .expect("child scope builds");

    assert_eq!(
        child
            .get::<AmbiguousLocalConsumer>()
            .expect("consumer built")
            .chosen
            .name(),
        "parent",
        "a complete ambiguous local set falls back to the parent primary"
    );
}

/// A trait exercising qualified single edges with no local qualifier match.
#[injectable]
trait QualChoice: Send + Sync {
    fn name(&self) -> &'static str;
}

/// The parent provider carrying the qualifier the consumer asks for.
#[component(provide = dyn QualChoice, qualifier = "wanted")]
struct WantedParentProvider;

impl QualChoice for WantedParentProvider {
    fn name(&self) -> &'static str {
        "wanted-parent"
    }
}

/// A child consumer with a qualified edge that matches no local provider.
#[component(scope = ChildScope)]
struct QualifiedChoiceConsumer {
    #[qualifier = "wanted"]
    chosen: Arc<dyn QualChoice>,
}

/// A local provider with a different qualifier, depending on the consumer: if
/// the consumer's qualified edge waited for every local provider (mistaking a
/// qualifier miss for ambiguity), the sort would report a false cycle.
#[component(
    scope = ChildScope,
    provide = dyn QualChoice,
    qualifier = "other"
)]
struct OtherLocalProvider {
    #[expect(unused)]
    consumer: Arc<QualifiedChoiceConsumer>,
}

impl QualChoice for OtherLocalProvider {
    fn name(&self) -> &'static str {
        "other-local"
    }
}

#[tokio::test]
async fn qualified_edge_without_local_match_does_not_wait_for_unrelated_locals() {
    let parent = <WantedParentProvider as Descriptor<ComponentDescriptor>>::DESCRIPTOR;
    let consumer = <QualifiedChoiceConsumer as Descriptor<ComponentDescriptor>>::DESCRIPTOR;
    let other = <OtherLocalProvider as Descriptor<ComponentDescriptor>>::DESCRIPTOR;
    let components = [parent, consumer, other];
    let providers: Vec<_> = PROVIDERS
        .iter()
        .filter(|provider| provider.trait_ty.type_id == TypeId::of::<dyn QualChoice>())
        .copied()
        .collect();
    let selection = Arc::new(
        ComponentRegistry {
            components: components.to_vec(),
            providers: providers.clone(),
        }
        .provider_selection_model(&components)
        .expect("provider selection model validates"),
    );
    let registry = Arc::new(
        ScopeRegistry::from_selection_model(
            HashMap::new(),
            components
                .iter()
                .map(|component| (component.ty.type_id, *component))
                .collect::<HashMap<TypeId, ComponentDescriptor>>(),
            Arc::clone(&selection),
        )
        .expect("scope registry validates"),
    );

    let root =
        ScopeContainer::build_root(&[parent], Vec::new(), ResolverSet::new(), registry.clone())
            .await
            .expect("root builds");
    let prebuilt: HashSet<TypeId> = [parent.ty.type_id].into_iter().collect();

    let child_components = [other, consumer];
    let order = topological_sort(&child_components, &prebuilt, &selection, can_reach)
        .expect("no false cycle through the unrelated local provider");
    let names: Vec<_> = order.iter().map(|component| component.name).collect();

    assert_eq!(names, ["QualifiedChoiceConsumer", "OtherLocalProvider"]);

    let order: Vec<ComponentDescriptor> = order.into_iter().copied().collect();
    let child = ScopeContainer::open_child(ChildScope::KEY, root, registry, &order, Vec::new())
        .await
        .expect("child scope builds");

    assert_eq!(
        child
            .get::<QualifiedChoiceConsumer>()
            .expect("consumer built")
            .chosen
            .name(),
        "wanted-parent",
        "a qualified edge with no local match resolves from the parent"
    );
}
