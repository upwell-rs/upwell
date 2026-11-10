//! Entry registration, the dense entry table, and target validation.

use std::sync::Arc;

use upwell_config::{Cfg, ConfigBinding, ConfigProperties};
use upwell_core::{
    EntryKey, RuntimeGenerationId, Scope, ScopeId, Singleton, Transient, TypeDescriptor,
};
use upwell_di::{Component, ComponentDescriptor};

use super::*;
use crate::scope::{PreparedScopeTopology, ScopeBoundary, ScopeParent, ScopeTopology};
use crate::test_support::{planned_provider, test_component};
use crate::transition::CandidateGraph;
use crate::{App, AppBuilder, AppRegistry, Error, ProtocolDefinition, ValidationContext};

const SESSION_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/entry-session");
const REQUEST_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/entry-request");
const UNDECLARED_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/entry-undeclared");

/// A long-lived scope opened under the root.
struct Session;
/// A short-lived scope opened under a session.
struct Request;
/// A scope the test topology does not declare.
struct Undeclared;

macro_rules! test_scope {
    ($type:ty, $id:expr, $rank:expr) => {
        impl Scope for $type {
            const ID: ScopeId = $id;
            const RANK: u8 = $rank;
            const NAME: &'static str = stringify!($type);
        }
    };
}

test_scope!(Session, SESSION_ID, 100);
test_scope!(Request, REQUEST_ID, 50);
test_scope!(Undeclared, UNDECLARED_ID, 75);

static BOUNDARIES: [ScopeBoundary; 2] = [
    ScopeBoundary::new::<Session>(&[ScopeParent::Root]),
    ScopeBoundary::new::<Request>(&[ScopeParent::of::<Session>()]),
];

/// A singleton every entry can reach.
struct Shared;
/// A session-scoped component.
struct SessionStore;
/// A request-scoped component.
struct RequestData;
/// A component no registry in these tests registers.
struct Unregistered;
/// The first of two tied providers of [`Greeter`].
struct English;
/// The second of two tied providers of [`Greeter`].
struct Swedish;

/// A trait whose providers tie unless one is primary.
trait Greeter: Send + Sync {}

test_component!(Shared, "shared");
test_component!(SessionStore, "session_store");
test_component!(RequestData, "request_data");
test_component!(Unregistered, "unregistered");
test_component!(English, "english", provides dyn Greeter);
test_component!(Swedish, "swedish", provides dyn Greeter);

impl Greeter for English {}
impl Greeter for Swedish {}

/// Config bound for config-target validation.
#[derive(serde::Deserialize)]
struct Limits;

impl ConfigProperties for Limits {
    const NAME: &'static str = "Limits";
}

/// A protocol opening the test session and request scopes.
#[derive(Default)]
struct EntryProtocol;

impl ProtocolDefinition for EntryProtocol {
    type Prepared = ();
    type Error = Error;

    const ID: crate::ProtocolId = upwell_core::namespaced_id!(crate::ProtocolId, "test/entries");
    const SCOPE_TOPOLOGY: ScopeTopology = ScopeTopology::new(&BOUNDARIES);

    fn register(&self, registry: &mut AppRegistry) {
        registry
            .entries
            .push(EntryDescriptor::new::<Request, ()>(key("test/protocol")));
    }

    fn prepare(self, _context: &ValidationContext<'_>) -> Result<Self::Prepared, Self::Error> {
        Ok(())
    }
}

fn builder() -> AppBuilder<EntryProtocol> {
    App::<EntryProtocol>::builder("entry-prepare-test")
        .config_source(upwell_config::ConfigManager::<upwell_config::Toml>::empty())
}

fn key(value: &'static str) -> EntryKey {
    EntryKey::new(value).expect("valid test entry key")
}

fn topology() -> PreparedScopeTopology {
    ScopeTopology::new(&BOUNDARIES)
        .prepare()
        .expect("test topology prepares")
}

fn manual<T: Component>(scope: upwell_core::ScopeKey) -> ComponentDescriptor {
    ComponentDescriptor::manual(T::ID, T::NAME, TypeDescriptor::of::<T>(T::NAME), scope)
}

fn greeter<T: Component>() -> upwell_di::ProviderDescriptor {
    planned_provider(
        TypeDescriptor::of::<dyn Greeter>("Greeter"),
        TypeDescriptor::of::<T>(T::NAME),
        T::ID,
        false,
    )
}

/// A registry of one component per test scope, plus `entries`.
fn registry(entries: Vec<EntryDescriptor>) -> AppRegistry {
    AppRegistry {
        components: vec![
            manual::<Shared>(Singleton::KEY),
            manual::<SessionStore>(Session::KEY),
            manual::<RequestData>(Request::KEY),
        ],
        entries,
        ..AppRegistry::default()
    }
}

/// Validates `registry`'s entries the way a reload candidate does.
fn validate(registry: &AppRegistry) -> crate::Result<()> {
    CandidateGraph::prepare(RuntimeGenerationId::INITIAL, registry, &topology()).map(|_| ())
}

fn entry_error(result: crate::Result<()>) -> EntryError {
    match result {
        Err(Error::Entry(error)) => error,
        Err(other) => panic!("expected an entry error, got {other}"),
        Ok(()) => panic!("expected an entry error, but validation passed"),
    }
}

mod prepare;
mod table;
mod targets;
#[cfg(feature = "tooling")]
mod tooling;
