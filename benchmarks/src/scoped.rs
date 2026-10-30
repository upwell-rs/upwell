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
use upwell_app::{App, AppBuilder, ProtocolDefinition};
use upwell_config::{ConfigManager, Dynamic};
use upwell_core::{ScopeKey, TypeDescriptor};
use upwell_di::{
    BoxedComponent, Component, ComponentConstructionContext, ComponentDescriptor,
    ComponentFactoryDescriptor, ComponentFactoryRole, Injectable, ScopeContainer,
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

fn construct<const N: usize>(
    _cx: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<CallScoped<N>>(CallScoped::<N>::NAME),
            value: Box::new(Injectable::into_stored(CallScoped::<N>.into_handle())),
        })
    })
}

fn factories<const N: usize>() -> &'static [ComponentFactoryDescriptor] {
    const {
        &[ComponentFactoryDescriptor {
            id: "explicit",
            construct: construct::<N>,
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
}

/// A built application with one open connection scope, opening per-call scopes under it.
pub struct ScopedCalls<D: ProtocolDefinition> {
    app: App<D>,
    connection: Arc<ScopeContainer>,
    per_call: ScopeKey,
}

impl<D: ProtocolDefinition> ScopedCalls<D> {
    /// Builds `builder` with `roster` in the `per_call` scope, then opens one `connection` scope
    /// from the root.
    pub async fn build(
        builder: AppBuilder<D>,
        connection: ScopeKey,
        per_call: ScopeKey,
        roster: Roster,
    ) -> Self
    where
        D::Error: std::fmt::Debug,
    {
        let app = roster
            .register(builder, per_call)
            .build()
            .await
            .expect("benchmark app builds");
        let connection = app
            .runtime()
            .open_scope_from_root(connection, Vec::new())
            .await
            .expect("benchmark connection scope opens");

        Self {
            app,
            connection,
            per_call,
        }
    }

    /// The scope a call's handler resolves through.
    pub async fn scope(&self, scope: CallScope) -> Arc<ScopeContainer> {
        match scope {
            CallScope::Connection => Arc::clone(&self.connection),

            CallScope::PerCall => self
                .app
                .runtime()
                .open_scope(self.per_call, Arc::clone(&self.connection), Vec::new())
                .await
                .expect("benchmark per-call scope opens"),
        }
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
/// - `fan_in/8`: the injection-free handler in a fresh per-call scope whose components include
///   one factory reading eight dependency edges.
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
        case(Roster::FanIn, CallScope::PerCall, Route::Hello),
    ));

    cases
}
