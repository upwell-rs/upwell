use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

use upwell_config::{ConfigManager, ConfigReloader};
use upwell_core::{ResolverSet, Scope, ScopeId, StaticScope, TypeDescriptor};
use upwell_di::{BoxedComponent, EffectiveGraph, ScopeContainer, ScopeRegistry};
use upwell_hooks::HookManager;

use super::*;
use crate::test_support::{empty_condition, seed_hook_manager};
use crate::{Error, ScopeBoundary, ScopeParent, ScopeTopology};

const SESSION_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/session");
const REQUEST_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/request");
const OTHER_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/other");

struct Session;
struct Request;

impl StaticScope for Session {
    const ID: ScopeId = SESSION_ID;
    const RANK: u8 = 100;
    const NAME: &'static str = "Session";
}

impl StaticScope for Request {
    const ID: ScopeId = REQUEST_ID;
    const RANK: u8 = 50;
    const NAME: &'static str = "Request";
}

static SESSION: Session = Session;
static REQUEST: Request = Request;
static OTHER: TestScope = TestScope::new(OTHER_ID, "Other", 25);
static REQUEST_ALIAS: TestScope = TestScope::new(REQUEST_ID, "Caller Metadata", 1);
static BOUNDARIES: [ScopeBoundary; 2] = [
    ScopeBoundary::new(&SESSION, ScopeParent::Root),
    ScopeBoundary::new(&REQUEST, ScopeParent::Boundary(SESSION_ID)),
];

/// A scope with test-controlled stable identity and metadata.
#[derive(Debug)]
struct TestScope {
    id: ScopeId,
    name: &'static str,
    rank: u8,
}

impl TestScope {
    const fn new(id: ScopeId, name: &'static str, rank: u8) -> Self {
        Self { id, name, rank }
    }
}

impl Scope for TestScope {
    fn id(&self) -> ScopeId {
        self.id
    }

    fn rank(&self) -> u8 {
        self.rank
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

/// A fresh empty generation over the test topology, prepared against `base`, plus the
/// scope registry its root was built from.
async fn prepared_generation(
    base: RuntimeGenerationId,
    seed_destinations: HashMap<TypeId, SeedDestination>,
) -> (PreparedRuntimeGeneration, Arc<ScopeRegistry>) {
    let registry = Arc::new(
        ScopeRegistry::new(HashMap::new(), HashMap::new(), Vec::new(), HashMap::new())
            .expect("empty scope registry validates"),
    );
    let root = ScopeContainer::build_root(
        &[],
        vec![seed_hook_manager(&HookManager::new(Vec::new()))],
        ResolverSet::new(),
        Arc::clone(&registry),
    )
    .await
    .expect("test root builds");
    let topology = ScopeTopology::new(&BOUNDARIES)
        .prepare()
        .expect("test topology prepares");
    let graph = EffectiveGraph::build(base, &upwell_di::ComponentRegistry::default(), |_, _| true)
        .expect("empty graph validates");
    let generation = PreparedRuntimeGeneration::new(
        root,
        Arc::clone(&registry),
        RuntimeScopePlan::new(
            Arc::new(topology),
            Arc::new(HashMap::from([
                (SESSION_ID, Vec::new()),
                (REQUEST_ID, Vec::new()),
            ])),
            Arc::new(seed_destinations),
        ),
        Arc::from([]),
        graph,
        empty_condition(),
    );

    (generation, registry)
}

async fn build_runtime(
    seed_destinations: HashMap<TypeId, SeedDestination>,
) -> (AppRuntime, Arc<ScopeRegistry>) {
    let (generation, registry) =
        prepared_generation(RuntimeGenerationId::INITIAL, seed_destinations).await;
    let reloader = ConfigReloader::new(ConfigManager::empty(), Vec::new());
    let runtime = AppRuntime::new(Arc::from("test"), generation, reloader);

    (runtime, registry)
}

/// Publishes a fresh empty generation over the runtime's current one.
async fn publish_next(runtime: &AppRuntime) -> RuntimeView {
    let transition = runtime.begin_transition().await;
    let (candidate, _) = prepared_generation(transition.base().id(), HashMap::new()).await;

    transition.publish(candidate).expect("candidate publishes")
}

fn seed<T: Send + Sync + 'static>() -> BoxedComponent {
    BoxedComponent {
        ty: TypeDescriptor::of::<T>(std::any::type_name::<T>()),
        value: Box::new(()),
    }
}

fn expect_app_error<T>(result: crate::Result<T>, message: &str) -> Error {
    match result {
        Ok(_) => panic!("{message}"),
        Err(error) => error,
    }
}

mod release;
mod scopes;
