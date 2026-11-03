//! Deterministic allocation contracts for one RPC call: what opening its per-call scope and
//! dispatching it allocates must follow what the method's entry plans, never the number of
//! components registered in the per-call scope.
//!
//! Each call opens its `Request` scope under one open connection scope exactly as the serve
//! loop does (`open_when_needed` with the method's entry), then dispatches through an
//! [`RpcRouter`]. The transport and middleware are left out: they do the same work for every
//! case. Allocation tracking is scoped to the test thread, and every call runs on it.

#[path = "../../di/src/test_support/alloc.rs"]
mod alloc;
#[path = "../../di/src/test_support/roster.rs"]
mod roster;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use upwell_app::{App, EntryDescriptor, OpenScope, ScopeEntry, ScopeOpener};
use upwell_config::{ConfigManager, Dynamic};
use upwell_core::{EntryKey, Scope, TypeDescriptor};
use upwell_di::Component;
use upwell_rpc::descriptors::{RpcCallContext, RpcHandler, RpcOutcome, RpcResponse};
use upwell_rpc::scope::{Connection, Request};
use upwell_rpc::{
    ErrorResponse, FromContext, Inject, OperationKind, ResolvedService, Rpc, RpcDescriptor,
    RpcGroup, RpcRouter, ServiceDescriptor,
};
use upwell_transport::PeerInfo;

use alloc::Delta;
use roster::{Scoped, UNUSED, per_call, roster, runtime};

/// A singleton a method injects, which a call reads without opening its scope.
struct Clock;

impl Component for Clock {
    type Handle = Arc<Self>;

    const ID: &'static str = "contract-clock";
    const NAME: &'static str = "Clock";

    fn into_handle(self) -> Arc<Self> {
        Arc::new(self)
    }
}

/// A type token naming the contract service.
struct ContractService;

/// What a method's handler resolves.
#[derive(Clone, Copy)]
enum Method {
    /// Nothing.
    Hello,
    /// `Inject<Arc<Scoped<0>>>`, a per-call component.
    Scoped,
    /// `Inject<Arc<Clock>>`, a singleton.
    Clock,
}

type Outcome = Pin<Box<dyn Future<Output = Result<RpcOutcome, ErrorResponse>> + Send>>;

impl Method {
    const ALL: [Self; 3] = [Self::Hello, Self::Scoped, Self::Clock];

    /// Whether an app with `count` per-call components can serve this method.
    fn served_with(self, count: usize) -> bool {
        !matches!(self, Self::Scoped) || count > 0
    }

    fn name(self) -> &'static str {
        match self {
            Self::Hello => "hello",
            Self::Scoped => "scoped",
            Self::Clock => "clock",
        }
    }

    fn path(self) -> String {
        format!("ContractService.{}", self.name())
    }

    fn key(self) -> EntryKey {
        let key = match self {
            Self::Hello => "rpc/contract.hello",
            Self::Scoped => "rpc/contract.scoped",
            Self::Clock => "rpc/contract.clock",
        };

        EntryKey::new(key).expect("contract entry keys are valid")
    }

    fn entry(self) -> EntryDescriptor {
        match self {
            Self::Hello => EntryDescriptor::new::<Request, ()>(self.key()),
            Self::Scoped => EntryDescriptor::new::<Request, (Arc<Scoped<0>>,)>(self.key()),
            Self::Clock => EntryDescriptor::new::<Request, (Arc<Clock>,)>(self.key()),
        }
    }

    fn handler(self) -> RpcHandler {
        match self {
            Self::Hello => hello,
            Self::Scoped => inject::<Arc<Scoped<0>>>,
            Self::Clock => inject::<Arc<Clock>>,
        }
    }

    fn descriptor(self) -> &'static RpcDescriptor {
        Box::leak(Box::new(RpcDescriptor {
            name: self.name(),
            operation: OperationKind::Unary,
            parameters: &[],
            output: TypeDescriptor::of::<()>("()"),
            handler: self.handler(),
            source: None,
            key: self.key(),
            targets: Vec::new,
        }))
    }
}

fn hello(_ctx: RpcCallContext) -> Outcome {
    Box::pin(async { Ok(RpcOutcome::Unary(RpcResponse::default())) })
}

fn inject<H: upwell_di::FromContainer + Send + 'static>(ctx: RpcCallContext) -> Outcome {
    Box::pin(async move {
        Inject::<H>::from_context(&ctx)
            .await
            .expect("the contract's target injects");

        Ok(RpcOutcome::Unary(RpcResponse::default()))
    })
}

fn no_groups() -> &'static [RpcGroup] {
    &[]
}

/// An RPC app with `count` per-call components and one open connection, opening each call's
/// scope as the serve loop does.
struct Calls {
    _app: App<Rpc>,
    connection: OpenScope<Connection>,
    calls: ScopeOpener<Request>,
    entries: Vec<(Method, ScopeEntry<Request>)>,
    router: RpcRouter,
}

impl Calls {
    async fn build(count: usize) -> Self {
        let builder = App::<Rpc>::builder("rpc-allocation-contracts")
            .config_source(ConfigManager::<Dynamic>::empty())
            .with_component(Clock);
        let builder = roster(Request::KEY, count)
            .iter()
            .fold(builder, |builder, component| {
                builder.component_descriptor(component)
            });
        let app = Method::ALL
            .into_iter()
            .filter(|method| method.served_with(count))
            .fold(builder, |builder, method| {
                builder.entry_descriptor(method.entry())
            })
            .build()
            .await
            .expect("the contract app builds");
        let connection = app
            .runtime()
            .opener::<Connection>()
            .expect("the connection scope is declared")
            .builder()
            .open_on_demand()
            .await
            .expect("the connection opens");
        let calls = app
            .runtime()
            .opener::<Request>()
            .expect("the call scope is declared");
        let entries = Method::ALL
            .into_iter()
            .filter(|method| method.served_with(count))
            .map(|method| {
                let entry = calls
                    .entry(method.key())
                    .expect("the method's entry resolves");

                (method, entry)
            })
            .collect();
        let service = ResolvedService {
            descriptor: ServiceDescriptor {
                id: "contract",
                name: "ContractService",
                ty: TypeDescriptor::of::<ContractService>("ContractService"),
                version: None,
                rpcs: no_groups,
            },
            rpcs: Method::ALL.into_iter().map(Method::descriptor).collect(),
        };

        Self {
            _app: app,
            connection,
            calls,
            entries,
            router: RpcRouter::from_services(std::slice::from_ref(&service)),
        }
    }

    async fn call(&self, method: Method) -> bool {
        let entry = self
            .entries
            .iter()
            .find_map(|(entry_method, entry)| {
                (entry_method.name() == method.name()).then_some(entry)
            })
            .expect("every method has an entry");
        let scope = self
            .calls
            .builder()
            .under(&self.connection)
            .open_when_needed(Some(entry))
            .await
            .expect("the call scope opens");
        let ctx = RpcCallContext::new(
            Vec::new(),
            PeerInfo { addr: None },
            scope,
            None,
            CancellationToken::new(),
        );

        self.router.dispatch(&method.path(), ctx).await.is_ok()
    }
}

/// What one call to `method` allocates in an app with `count` per-call components.
fn measured(count: usize, method: Method) -> Delta {
    let runtime = runtime();
    let calls = runtime.block_on(Calls::build(count));

    per_call(&runtime, || calls.call(method))
}

#[test]
fn unused_per_call_components_cost_a_call_nothing() {
    let without = measured(0, Method::Hello);
    let with_unused = measured(UNUSED, Method::Hello);

    assert_eq!(
        with_unused, without,
        "{UNUSED} unused per-call components changed what a call injecting nothing allocates"
    );
}

#[test]
fn a_call_injecting_one_component_costs_its_plan_not_the_scope() {
    let alone = measured(1, Method::Scoped);
    let among_unused = measured(1 + UNUSED, Method::Scoped);

    assert_eq!(
        among_unused, alone,
        "{UNUSED} unused per-call components changed what a call injecting one allocates"
    );
}

/// Opening a call scope always allocates its container, so a call that opens none makes as many
/// allocations as one injecting nothing. Its bytes differ by the size of its handler's future.
#[test]
fn a_call_injecting_a_singleton_opens_no_scope() {
    let hello = measured(UNUSED, Method::Hello);
    let clock = measured(UNUSED, Method::Clock);

    assert_eq!(
        clock.allocations, hello.allocations,
        "injecting a planned singleton allocated more often than injecting nothing"
    );
}
