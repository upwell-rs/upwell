//! A fixed consumer of the switched `Authenticator` provider: it injects
//! `Arc<dyn Authenticator>` once at construction, so a provider switch must reconstruct it
//! rather than retain a stale provider.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use upwell_core::{DependencyDescriptor, TypeDescriptor};
use upwell_di::{
    BoxedComponent, Component, ComponentConstructionContext, ComponentDescriptor,
    ComponentFactoryDescriptor, FromContainer, Injectable, Singleton,
};

use super::fixture::Authenticator;

/// A singleton gate that captured the effective authenticator's scheme at construction.
pub(super) struct AuthGate {
    pub(super) scheme: &'static str,
}

impl Component for AuthGate {
    type Handle = Arc<Self>;

    const ID: &'static str = "auth-gate";
    const NAME: &'static str = "AuthGate";

    fn into_handle(self) -> Self::Handle {
        Arc::new(self)
    }
}

fn gate_dependencies() -> Vec<DependencyDescriptor> {
    vec![<Arc<dyn Authenticator> as FromContainer>::dependency()]
}

/// The boxed factory future the erased [`ComponentFactoryDescriptor`] signature returns.
type GateFactoryFuture<'a> =
    Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + 'a>>;

fn construct_gate(context: &mut ComponentConstructionContext) -> GateFactoryFuture<'_> {
    Box::pin(async move {
        let authenticator =
            <Arc<dyn Authenticator> as FromContainer>::from_container(context).await?;

        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<AuthGate>(AuthGate::NAME),
            value: Box::new(Injectable::into_stored(Arc::new(AuthGate {
                scheme: authenticator.scheme(),
            }))),
        })
    })
}

fn gate_factories() -> &'static [ComponentFactoryDescriptor] {
    &[ComponentFactoryDescriptor {
        id: "static",
        construct: construct_gate,
        dependencies: gate_dependencies,
        default: true,
    }]
}

pub(super) static GATE_COMPONENT: ComponentDescriptor = ComponentDescriptor {
    id: AuthGate::ID,
    name: AuthGate::NAME,
    ty: TypeDescriptor::of::<AuthGate>(AuthGate::NAME),
    scope: &Singleton,
    condition: None,
    factories: gate_factories,
    hooks: upwell_hooks::no_hooks,
    generation_snapshot: None,
};
