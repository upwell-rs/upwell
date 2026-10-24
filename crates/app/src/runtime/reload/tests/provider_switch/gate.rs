//! A fixed consumer of the switched `Authenticator` provider: it injects
//! `Arc<dyn Authenticator>` once at construction, so a provider switch must reconstruct it
//! rather than retain a stale provider.

use std::sync::Arc;

use upwell_di::{ComponentDescriptor, ComponentFactoryDescriptor, factories};

use super::fixture::Authenticator;
use crate::test_support::test_component;

/// A singleton gate that captured the effective authenticator's scheme at construction.
pub(super) struct AuthGate {
    pub(super) scheme: &'static str,
}

test_component!(AuthGate, "auth-gate");

async fn construct_gate(authenticator: Arc<dyn Authenticator>) -> AuthGate {
    AuthGate {
        scheme: authenticator.scheme(),
    }
}

pub(super) const GATE_FACTORIES: fn() -> &'static [ComponentFactoryDescriptor] =
    factories!["static" => construct_gate];

pub(super) static GATE_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<AuthGate>()
    .with_factories(GATE_FACTORIES)
    .without_retention();
