use std::{any::TypeId, collections::HashMap, sync::Arc};

use upwell_core::{ResolverSet, ScopeId, StaticScope, Transient, TypeDescriptor};

use super::fresh::{fresh_arc, fresh_construct_all};
use super::*;
use crate::container::ScopeResolverSlot;
use crate::error::Error;
use crate::test_support::test_component;
use crate::{
    BoxedComponent, Component, ComponentDescriptor, Injectable, Live, ProviderDescriptor,
    ProviderOf, ScopeContainer, ScopeRegistry, factories,
};

const VISIBLE_SCOPE_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/fresh-visible");
const SIBLING_SCOPE_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/fresh-sibling");

/// The branch from which fresh providers are resolved.
struct VisibleScope;

impl StaticScope for VisibleScope {
    const ID: ScopeId = VISIBLE_SCOPE_ID;
    const RANK: u8 = 1;
    const NAME: &'static str = "Visible";
}

/// An inaccessible sibling of [`VisibleScope`].
struct SiblingScope;

impl StaticScope for SiblingScope {
    const ID: ScopeId = SIBLING_SCOPE_ID;
    const RANK: u8 = 1;
    const NAME: &'static str = "Sibling";
}

/// Trait used to distinguish fresh providers across sibling branches.
trait BranchProvider: Send + Sync {
    fn source(&self) -> &'static str;
}

/// The provider declared in the visible branch.
struct VisibleProvider;

impl BranchProvider for VisibleProvider {
    fn source(&self) -> &'static str {
        "visible"
    }
}

/// The globally primary provider declared in the inaccessible sibling branch.
struct SiblingPrimaryProvider;

impl BranchProvider for SiblingPrimaryProvider {
    fn source(&self) -> &'static str {
        "sibling"
    }
}

/// A transient provider that is visible from every scope.
struct TransientProvider;

impl BranchProvider for TransientProvider {
    fn source(&self) -> &'static str {
        "transient"
    }
}

/// A factory-less provider declared in an inaccessible sibling branch.
struct SiblingSeedProvider;

impl BranchProvider for SiblingSeedProvider {
    fn source(&self) -> &'static str {
        "sibling-seed"
    }
}

test_component!(VisibleProvider, "visible-provider", provides dyn BranchProvider);
test_component!(SiblingPrimaryProvider, "sibling-primary-provider", provides dyn BranchProvider);
test_component!(TransientProvider, "transient-provider", provides dyn BranchProvider);
test_component!(SiblingSeedProvider, "sibling-seed-provider", provides dyn BranchProvider);

async fn visible() -> VisibleProvider {
    VisibleProvider
}

async fn sibling() -> SiblingPrimaryProvider {
    SiblingPrimaryProvider
}

async fn transient() -> TransientProvider {
    TransientProvider
}

fn visible_descriptor() -> ComponentDescriptor {
    ComponentDescriptor::of::<VisibleProvider>()
        .with_scope(&VisibleScope)
        .with_factories(factories!["static" => visible])
}

fn sibling_descriptor() -> ComponentDescriptor {
    ComponentDescriptor::of::<SiblingPrimaryProvider>()
        .with_scope(&SiblingScope)
        .with_factories(factories!["static" => sibling])
}

fn transient_descriptor() -> ComponentDescriptor {
    ComponentDescriptor::of::<TransientProvider>()
        .with_scope(&Transient)
        .with_factories(factories!["static" => transient])
}

/// The non-primary mapping of `T` to `dyn BranchProvider` under the repeated `"shared"`
/// qualifier.
fn branch_provider<T>() -> ProviderDescriptor
where
    T: Component + ProviderOf<dyn BranchProvider>,
    T::Handle: Injectable<Stored = Live<T>>,
{
    ProviderDescriptor::of::<T, dyn BranchProvider>("dyn BranchProvider").qualifier("shared")
}

async fn sibling_branches() -> (Arc<ScopeContainer>, Arc<ScopeContainer>) {
    let visible = visible_descriptor();
    let sibling = sibling_descriptor();
    let sibling_seed = ComponentDescriptor::manual(
        "sibling-seed-provider",
        "SiblingSeedProvider",
        TypeDescriptor::of::<SiblingSeedProvider>("SiblingSeedProvider"),
        &SiblingScope,
    );
    let components = [visible, sibling, sibling_seed]
        .into_iter()
        .map(|descriptor| (descriptor.ty.type_id, descriptor))
        .collect::<HashMap<TypeId, ComponentDescriptor>>();
    let providers = vec![
        branch_provider::<SiblingPrimaryProvider>().primary(),
        branch_provider::<VisibleProvider>(),
        branch_provider::<SiblingSeedProvider>(),
    ];
    let registry = Arc::new(
        ScopeRegistry::new(HashMap::new(), components, providers, HashMap::new())
            .expect("scope registry validates"),
    );
    let root =
        ScopeContainer::build_root(&[], Vec::new(), ResolverSet::new(), Arc::clone(&registry))
            .await
            .expect("root builds");
    let visible_branch = ScopeContainer::open_child(
        &VisibleScope,
        Arc::clone(&root),
        Arc::clone(&registry),
        &[],
        Vec::new(),
    )
    .await
    .expect("visible branch opens");
    let sibling_branch = ScopeContainer::open_child(&SiblingScope, root, registry, &[], Vec::new())
        .await
        .expect("sibling branch opens");

    (visible_branch, sibling_branch)
}

async fn transient_and_inaccessible_provider() -> Arc<ScopeContainer> {
    let transient = transient_descriptor();
    let sibling = sibling_descriptor();
    let transient_components = [(transient.ty.type_id, transient)].into_iter().collect();
    let factory_backed = [transient, sibling]
        .into_iter()
        .map(|descriptor| (descriptor.ty.type_id, descriptor))
        .collect();
    let providers = vec![
        branch_provider::<SiblingPrimaryProvider>().primary(),
        branch_provider::<TransientProvider>(),
    ];
    let registry = Arc::new(
        ScopeRegistry::new(
            transient_components,
            factory_backed,
            providers,
            HashMap::new(),
        )
        .expect("scope registry validates"),
    );
    let root =
        ScopeContainer::build_root(&[], Vec::new(), ResolverSet::new(), Arc::clone(&registry))
            .await
            .expect("root builds");

    ScopeContainer::open_child(&VisibleScope, root, registry, &[], Vec::new())
        .await
        .expect("visible branch opens")
}

async fn accessible_factoryless_provider() -> Arc<ScopeContainer> {
    let manual = ComponentDescriptor::manual(
        "visible-seed-provider",
        "VisibleSeedProvider",
        TypeDescriptor::of::<SiblingSeedProvider>("VisibleSeedProvider"),
        &VisibleScope,
    );
    let components = [(manual.ty.type_id, manual)].into_iter().collect();
    let providers = vec![branch_provider::<SiblingSeedProvider>()];
    let registry = Arc::new(
        ScopeRegistry::new(HashMap::new(), components, providers, HashMap::new())
            .expect("scope registry validates"),
    );
    let root =
        ScopeContainer::build_root(&[], Vec::new(), ResolverSet::new(), Arc::clone(&registry))
            .await
            .expect("root builds");
    let seed = BoxedComponent::new(SiblingSeedProvider);

    ScopeContainer::open_child(&VisibleScope, root, registry, &[], vec![seed])
        .await
        .expect("visible branch opens")
}

#[test]
fn deferred_panics_before_scope_hydration() {
    let deferred = Deferred::<u8>::capture(ScopeResolverSlot::default(), None)
        .expect("deferred slot registers");

    assert!(deferred.try_get().is_none());
    assert!(std::panic::catch_unwind(|| deferred.get()).is_err());
}

#[tokio::test]
async fn unattached_scope_returns_typed_error_for_lazy() {
    let lazy = Lazy::<Arc<u8>>::capture(ScopeResolverSlot::default());

    assert!(matches!(
        lazy.get_or_create().await,
        Err(Error::ScopeUnavailable)
    ));
}

#[tokio::test]
async fn fresh_ignores_primary_provider_from_inaccessible_sibling() {
    let (visible_branch, sibling_branch) = sibling_branches().await;
    let provider = fresh_arc::<dyn BranchProvider>(&visible_branch, None)
        .await
        .expect("fresh resolution succeeds")
        .expect("visible provider resolves");

    assert_eq!(visible_branch.scope().id(), VISIBLE_SCOPE_ID);
    assert_eq!(sibling_branch.scope().id(), SIBLING_SCOPE_ID);
    assert_eq!(provider.source(), "visible");
}

#[tokio::test]
async fn qualified_fresh_selects_repeated_qualifier_from_visible_sibling() {
    let (visible_branch, sibling_branch) = sibling_branches().await;
    let provider = fresh_arc::<dyn BranchProvider>(&visible_branch, Some("shared"))
        .await
        .expect("qualified fresh resolution succeeds")
        .expect("visible qualified provider resolves");

    assert_eq!(visible_branch.scope().id(), VISIBLE_SCOPE_ID);
    assert_eq!(sibling_branch.scope().id(), SIBLING_SCOPE_ID);
    assert_eq!(provider.source(), "visible");
}

#[tokio::test]
async fn eager_resolution_selects_visible_transient_before_inaccessible_primary() {
    let visible_branch = transient_and_inaccessible_provider().await;
    let provider = visible_branch
        .resolve::<Arc<dyn BranchProvider>>()
        .await
        .expect("resolution succeeds")
        .expect("transient provider resolves");

    assert_eq!(provider.source(), "transient");
}

#[tokio::test]
async fn fresh_collection_ignores_inaccessible_factoryless_provider() {
    let (visible_branch, _) = sibling_branches().await;
    let providers = fresh_construct_all::<dyn BranchProvider>(&visible_branch)
        .await
        .expect("fresh collection resolves");
    let sources: Vec<_> = providers
        .into_iter()
        .map(|(_, provider)| provider.source())
        .collect();

    assert_eq!(sources, ["visible"]);
}

#[tokio::test]
async fn fresh_collection_rejects_accessible_factoryless_provider() {
    let visible_branch = accessible_factoryless_provider().await;
    let error = match <Vec<Arc<dyn BranchProvider>> as FreshFromContainer>::fresh_from_container(
        visible_branch,
        None,
    )
    .await
    {
        Ok(_) => panic!("visible factory-less provider cannot be reconstructed"),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        Error::UnsupportedFreshFactory {
            component_id: Some(component_id),
            ..
        } if component_id == "visible-seed-provider"
    ));
}

#[tokio::test]
async fn fresh_keyed_rejects_accessible_factoryless_provider() {
    let visible_branch = accessible_factoryless_provider().await;
    let error = match
        <HashMap<String, Arc<dyn BranchProvider>> as FreshFromContainer>::fresh_from_container(
            visible_branch,
            None,
        )
        .await
    {
        Ok(_) => panic!("visible factory-less provider cannot be reconstructed"),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        Error::UnsupportedFreshFactory {
            component_id: Some(component_id),
            ..
        } if component_id == "visible-seed-provider"
    ));
}
