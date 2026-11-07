//! Injecting a [`ScopeOpener`] as a parameter.

use std::marker::PhantomData;

use upwell_core::{DependencyDescriptor, Scope};
use upwell_di::{ComponentConstructionContext, FromContainer, ScopeContainer};

use super::{OpenerRuntime, ScopeOpener};
use crate::builtins::RuntimeReloader;

/// An injected opener reaches its runtime through the framework-seeded [`RuntimeReloader`],
/// which every application provides and which holds the runtime weakly. It can be injected
/// while the application builds, before its runtime exists, and checks that the runtime
/// declares `S` when it opens an instance.
impl<S: Scope> FromContainer for ScopeOpener<S> {
    fn dependency() -> DependencyDescriptor {
        <RuntimeReloader as FromContainer>::dependency()
    }

    async fn from_container(cx: &ComponentConstructionContext) -> upwell_di::Result<Self> {
        RuntimeReloader::from_container(cx).await.map(injected)
    }

    fn read_built(scope: &ScopeContainer) -> Option<Self> {
        RuntimeReloader::read_built(scope).map(injected)
    }
}

fn injected<S: Scope>(reloader: RuntimeReloader) -> ScopeOpener<S> {
    ScopeOpener {
        runtime: OpenerRuntime::Injected(reloader),
        scope: PhantomData,
    }
}
