//! Per-call scope fixtures shared by every protocol bench.
//!
//! [`CallScoped`] is a roster of zero-sized components that can be registered in any scope, so a
//! bench can fill a protocol's per-call scope with components its handlers never use.
//! [`ScopedCalls`] is a real application plus one open connection scope, and opens the protocol's
//! per-call scope under it exactly as the protocol's dispatch loop does. [`cases`] is the case set
//! every connection-oriented protocol is measured against.
//!
//! Each case builds its own application, because a per-call scope currently constructs every
//! component registered in it: the component count is part of the case, not a shared backdrop.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::runtime::Runtime;
use upwell_app::{
    App, AppBuilder, EntryDescriptor, EntryScope, OpenScope, OpensUnder, ProtocolDefinition,
    ScopeEntry, ScopeOpener,
};
use upwell_config::{ConfigManager, Dynamic};
use upwell_core::{EntryKey, Scope, ScopeKey, Singleton, TypeDescriptor};
use upwell_di::{
    BoxedComponent, Component, ComponentConstructionContext, ComponentDescriptor,
    ComponentFactoryDescriptor, ComponentFactoryRole,
};

use crate::cases::{Call, Case};
use crate::fan_in::{self, FAN_IN};

/// The largest number of scoped components a fixture can register.
pub const MAX_SCOPED: usize = 64;

/// Unused scoped component counts; 10 matches the regression report in #343.
pub const UNUSED: [usize; 3] = [1, 10, 64];

/// A zero-sized scoped component, distinct per `N`, so its cost is the container's alone.
pub struct CallScoped<const N: usize>;

impl<const N: usize> Component for CallScoped<N> {
    type Handle = Arc<Self>;

    const ID: &'static str = "call-scoped-component";
    const NAME: &'static str = "CallScopedComponent";

    fn into_handle(self) -> Arc<Self> {
        Arc::new(self)
    }
}

/// Boxes a [`CallScoped`] as `cx`'s scope stores it, like a generated factory.
fn boxed<const N: usize>(cx: &ComponentConstructionContext) -> BoxedComponent {
    BoxedComponent {
        ty: TypeDescriptor::of::<CallScoped<N>>(CallScoped::<N>::NAME),
        value: Box::new(cx.stored(CallScoped::<N>.into_handle())),
    }
}

fn construct<const N: usize>(
    cx: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async { Ok(boxed::<N>(cx)) })
}

fn construct_ready<const N: usize>(
    cx: &ComponentConstructionContext,
) -> upwell_di::Result<Option<BoxedComponent>> {
    Ok(Some(boxed::<N>(cx)))
}

fn factories<const N: usize>() -> &'static [ComponentFactoryDescriptor] {
    const {
        &[ComponentFactoryDescriptor {
            id: "explicit",
            construct: construct::<N>,
            construct_ready: Some(construct_ready::<N>),
            dependencies: Vec::new,
            role: ComponentFactoryRole::Explicit,
        }]
    }
}

fn call_scoped<const N: usize>(scope: ScopeKey) -> ComponentDescriptor {
    ComponentDescriptor {
        id: Box::leak(format!("call-scoped-component-{N}").into_boxed_str()),
        scope,
        factories: factories::<N>,
        ..ComponentDescriptor::of::<CallScoped<N>>()
    }
}

macro_rules! scoped_roster {
    ($scope:expr; $($n:literal),* $(,)?) => {
        vec![ $( call_scoped::<$n>($scope) ),* ]
    };
}

/// Every roster descriptor in `scope`, in index order. Leaked to `'static` because the app
/// builder borrows descriptors for the program's lifetime, which a benchmark can afford.
fn roster(scope: ScopeKey) -> &'static [ComponentDescriptor] {
    let roster = scoped_roster!(
        scope;
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
        25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46,
        47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63
    );

    Box::leak(roster.into_boxed_slice())
}

/// Registers the first `count` roster components in `scope` with `builder`.
///
/// # Panics
///
/// Panics when `count` exceeds [`MAX_SCOPED`], the size of the roster.
#[must_use]
pub fn register<D: ProtocolDefinition>(
    builder: AppBuilder<D>,
    scope: ScopeKey,
    count: usize,
) -> AppBuilder<D> {
    assert!(
        count <= MAX_SCOPED,
        "fixture needs {count} scoped components but the roster holds {MAX_SCOPED}"
    );

    roster(scope)[..count]
        .iter()
        .fold(builder, |builder, descriptor| {
            builder.component_descriptor(descriptor)
        })
}

/// The components a fixture registers in its per-call scope.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub enum Roster {
    /// The first `N` roster components, none of which has a dependency.
    Unused(usize),

    /// [`FAN_IN`] roster components plus a [`FanIn`](fan_in::FanIn) consumer depending on all
    /// of them, so every per-call scope runs a factory reading [`FAN_IN`] edges.
    FanIn,
}

impl Roster {
    /// Registers this roster in `scope` with `builder`.
    ///
    /// # Panics
    ///
    /// Panics when `self` is [`Roster::Unused`] with a count above [`MAX_SCOPED`].
    #[must_use]
    pub fn register<D: ProtocolDefinition>(
        self,
        builder: AppBuilder<D>,
        scope: ScopeKey,
    ) -> AppBuilder<D> {
        match self {
            Self::Unused(count) => register(builder, scope, count),

            Self::FanIn => {
                register(builder, scope, FAN_IN).component_descriptor(fan_in::descriptor(scope))
            }
        }
    }
}

/// Builds an application named `name` with an empty config source.
#[must_use]
pub fn builder<D: ProtocolDefinition>(name: &str) -> AppBuilder<D> {
    App::<D>::builder(name).config_source(ConfigManager::<Dynamic>::empty())
}

/// Which scope a call's handler resolves through.
#[derive(Clone, Copy)]
pub enum CallScope {
    /// The handler resolves through the connection scope and no per-call scope is opened. This is
    /// the protocol's dispatch cost without per-call DI, the baseline every other case is read
    /// against.
    Connection,

    /// A fresh per-call scope is opened under the connection scope, as the protocol does.
    PerCall,
}

/// The handler a call invokes.
#[derive(Clone, Copy)]
pub enum Route {
    /// A handler that resolves nothing.
    Hello,

    /// A handler that resolves [`CallScoped<0>`] from its scope.
    Scoped,

    /// A handler whose entry injects the [`FanIn`](fan_in::FanIn) consumer, so its call builds
    /// the consumer and every edge it reads. The handler itself resolves nothing.
    FanIn,
}

impl Route {
    /// The stable key of this route's entry.
    fn key(self) -> EntryKey {
        let key = match self {
            Self::Hello => "bench/hello",
            Self::Scoped => "bench/scoped",
            Self::FanIn => "bench/fan-in",
        };

        EntryKey::new(key).expect("bench entry keys are valid")
    }

    /// The entry of this route in scope `P`, when `roster` registers what it injects.
    fn entry<P: Scope>(self, roster: Roster) -> Option<EntryDescriptor> {
        let key = self.key();

        match (self, roster) {
            (Self::Hello, _) => Some(EntryDescriptor::new::<P, ()>(key)),
            (Self::Scoped, Roster::Unused(0)) => None,
            (Self::Scoped, _) => Some(EntryDescriptor::new::<P, (Arc<CallScoped<0>>,)>(key)),
            (Self::FanIn, Roster::FanIn) => {
                Some(EntryDescriptor::new::<P, (Arc<fan_in::FanIn>,)>(key))
            }
            (Self::FanIn, Roster::Unused(_)) => None,
        }
    }
}

/// The entry each [`Route`] opens its per-call scope for, when its roster registers one.
struct RouteEntries<P: Scope> {
    hello: Option<ScopeEntry<P>>,
    scoped: Option<ScopeEntry<P>>,
    fan_in: Option<ScopeEntry<P>>,
}

impl<P: Scope> RouteEntries<P> {
    fn new(opener: &ScopeOpener<P>, roster: Roster) -> Self {
        let entry = |route: Route| {
            route.entry::<P>(roster).map(|_| {
                opener
                    .entry(route.key())
                    .expect("the bench registers every route entry it opens")
            })
        };

        Self {
            hello: entry(Route::Hello),
            scoped: entry(Route::Scoped),
            fan_in: entry(Route::FanIn),
        }
    }

    fn get(&self, route: Route) -> Option<&ScopeEntry<P>> {
        match route {
            Route::Hello => self.hello.as_ref(),
            Route::Scoped => self.scoped.as_ref(),
            Route::FanIn => self.fan_in.as_ref(),
        }
    }
}

/// A built application with one open connection scope `C`, opening per-call scopes `P` under
/// it through the same prepared openers a protocol's dispatch loop holds.
pub struct ScopedCalls<D: ProtocolDefinition, C: Scope, P: Scope> {
    _app: App<D>,
    connection: OpenScope<C>,
    per_call: ScopeOpener<P>,
    entries: RouteEntries<P>,
}

impl<D, C, P> ScopedCalls<D, C, P>
where
    D: ProtocolDefinition,
    C: OpensUnder<Singleton>,
    P: OpensUnder<C>,
{
    /// Builds `builder` with `roster` in the per-call scope, then opens one connection scope
    /// from the root.
    pub async fn build(builder: AppBuilder<D>, roster: Roster) -> Self
    where
        D::Error: std::fmt::Debug,
    {
        let routes = [Route::Hello, Route::Scoped, Route::FanIn];
        let builder = routes
            .into_iter()
            .filter_map(|route| route.entry::<P>(roster))
            .fold(
                roster.register(builder, P::KEY),
                AppBuilder::entry_descriptor,
            );
        let app = builder.build().await.expect("benchmark app builds");
        let connection = app
            .runtime()
            .opener::<C>()
            .expect("the connection scope is declared")
            .builder()
            .open_on_demand()
            .await
            .expect("benchmark connection scope opens");
        let per_call = app
            .runtime()
            .opener::<P>()
            .expect("the per-call scope is declared");
        let entries = RouteEntries::new(&per_call, roster);

        Self {
            _app: app,
            connection,
            per_call,
            entries,
        }
    }

    /// The scope a call to `route` resolves through. A per-call scope opens for the route's
    /// entry as a protocol opens it for a routed call: deferred when the entry plans nothing.
    pub async fn scope(&self, scope: CallScope, route: Route) -> EntryScope<P> {
        if let CallScope::Connection = scope {
            return EntryScope::from_container(Arc::clone(self.connection.container()));
        }

        let builder = self.per_call.builder().under(&self.connection);
        let opened = match self.entries.get(route) {
            Some(entry) => builder.open_when_needed(entry).await,
            None => builder
                .open_default()
                .await
                .map(|scope| EntryScope::from_container(scope.into_container())),
        };

        opened.expect("benchmark per-call scope opens")
    }
}

/// A protocol fixture that serves the [`Route`]s over [`ScopedCalls`].
pub trait ScopedProtocol: Sized {
    /// Builds the fixture with `roster` in its per-call scope.
    fn build(roster: Roster) -> impl Future<Output = Self>;

    /// Dispatches one call to `route` through `scope` and returns whether it succeeded.
    fn call(&self, scope: CallScope, route: Route) -> impl Future<Output = bool>;
}

/// One call of a [`ScopedProtocol`] case.
pub struct ScopedCall<P> {
    fixture: P,
    scope: CallScope,
    route: Route,
}

impl<P: ScopedProtocol> Call for ScopedCall<P> {
    fn call(&self) -> impl Future<Output = bool> {
        self.fixture.call(self.scope, self.route)
    }
}

/// The case set every connection-oriented protocol is measured against:
///
/// - `dispatch_only`: the injection-free handler on the connection scope, with no per-call scope.
/// - `empty_scope`: the same handler in a fresh per-call scope with no components registered.
/// - `scoped`: a handler resolving one per-call component, the only one registered.
/// - `unused/N`: the injection-free handler in a fresh per-call scope with `N` registered
///   components it never uses.
/// - `fan_in/8`: a handler whose entry injects one factory reading eight dependency edges, in a
///   fresh per-call scope.
pub fn cases<P: ScopedProtocol>(runtime: &Runtime) -> Vec<Case<ScopedCall<P>>> {
    let case = |roster, scope, route| ScopedCall {
        fixture: runtime.block_on(P::build(roster)),
        scope,
        route,
    };
    let mut cases = vec![
        Case::new(
            "dispatch_only",
            case(Roster::Unused(0), CallScope::Connection, Route::Hello),
        ),
        Case::new(
            "empty_scope",
            case(Roster::Unused(0), CallScope::PerCall, Route::Hello),
        ),
        Case::new(
            "scoped",
            case(Roster::Unused(1), CallScope::PerCall, Route::Scoped),
        ),
    ];

    for count in UNUSED {
        cases.push(Case::new(
            format!("unused/{count}"),
            case(Roster::Unused(count), CallScope::PerCall, Route::Hello),
        ));
    }

    cases.push(Case::new(
        format!("fan_in/{FAN_IN}"),
        case(Roster::FanIn, CallScope::PerCall, Route::FanIn),
    ));

    cases
}
