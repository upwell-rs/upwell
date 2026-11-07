//! Scope chains, targets and contexts shared by the invocation-context tests.

use std::collections::HashMap;
use std::sync::Arc;

use upwell_core::{ResolverSet, Scope, ScopeId, TypeDescriptor};

use super::{ContextError, ContextRuntime, InvocationContext, InvocationTarget, TargetRef};
use crate::container::{ScopeContainer, ScopeRegistry};
use crate::descriptors::{BoxedComponent, Injectable};

/// A request-like scope opened under the root.
pub(crate) struct Request;

/// A message-like scope opened under [`Request`].
pub(crate) struct Message;

impl Scope for Request {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/context-request");
    const RANK: u8 = 1;
    const NAME: &'static str = "Request";
}

impl Scope for Message {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/context-message");
    const RANK: u8 = 2;
    const NAME: &'static str = "Message";
}

/// The entry points the tests establish contexts for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Entry {
    Http,
    Connection,
    Message,
    Rpc,
    Hook,
}

impl InvocationTarget for Entry {}

/// A target a third-party protocol defines, carrying data.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Custom {
    pub(crate) name: &'static str,
}

impl InvocationTarget for Custom {}

/// A root and a request instance under it.
pub(crate) struct Chain {
    pub(crate) root: Arc<ScopeContainer>,
    pub(crate) request: Arc<ScopeContainer>,
}

pub(crate) async fn chain() -> Chain {
    let registry = Arc::new(
        ScopeRegistry::new(HashMap::new(), HashMap::new(), Vec::new(), HashMap::new())
            .expect("empty scope registry validates"),
    );
    let root =
        ScopeContainer::build_root(&[], Vec::new(), ResolverSet::new(), Arc::clone(&registry))
            .await
            .expect("root builds");
    let request = child::<Request>(&root).await;

    Chain { root, request }
}

pub(crate) async fn child<S: Scope>(parent: &Arc<ScopeContainer>) -> Arc<ScopeContainer> {
    seeded::<S>(parent, Vec::new()).await
}

/// An instance of `S` under `parent` holding `seeds`.
pub(crate) async fn seeded<S: Scope>(
    parent: &Arc<ScopeContainer>,
    seeds: Vec<BoxedComponent>,
) -> Arc<ScopeContainer> {
    ScopeContainer::open_child(S::KEY, Arc::clone(parent), parent.registry(), &[], seeds)
        .await
        .expect("a child scope opens")
}

/// A seed holding `value` as an `Arc<T>` component.
pub(crate) fn seed<T: Send + Sync + 'static>(value: T) -> BoxedComponent {
    BoxedComponent::stored(
        TypeDescriptor::of::<T>(std::any::type_name::<T>()),
        Arc::new(value).into_fixed(),
    )
}

/// The chain's root as the runtime a context runs on.
pub(crate) fn runtime(chain: &Chain) -> Arc<dyn ContextRuntime> {
    Arc::clone(&chain.root) as Arc<dyn ContextRuntime>
}

pub(crate) fn context(chain: &Chain, entry: &'static Entry) -> InvocationContext {
    InvocationContext::new(TargetRef::from_static(entry), runtime(chain))
}

pub(crate) fn innermost() -> Result<Arc<ScopeContainer>, ContextError> {
    InvocationContext::with_current(InvocationContext::scope)?
}
