//! Per-call scoped components and a per-call allocation measurement, shared by the protocols'
//! `allocation_contracts` tests.
//!
//! Not part of this crate's own tests: each contract binary includes this file next to
//! `alloc.rs` (`#[path]`), which it must name `alloc` at its crate root.

#![allow(dead_code)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use upwell_core::{ScopeKey, TypeDescriptor};
use upwell_di::{
    BoxedComponent, Component, ComponentConstructionContext, ComponentDescriptor,
    ComponentFactoryDescriptor, ComponentFactoryRole,
};

use super::alloc::{Delta, measure};

/// The scoped components a fixture leaves unused next to the one it injects.
pub(crate) const UNUSED: usize = 64;

/// Calls measured per case, after [`WARMUP`] unmeasured ones.
const CALLS: usize = 32;

/// Unmeasured calls that settle lazily initialised state (thread locals, caches, tracing
/// callsites) before a case is measured.
const WARMUP: usize = 4;

/// A zero-sized scoped component, distinct per `N`, so its cost is the container's alone.
pub(crate) struct Scoped<const N: usize>;

impl<const N: usize> Component for Scoped<N> {
    type Handle = Arc<Self>;

    const ID: &'static str = "contract-scoped";
    const NAME: &'static str = "ContractScoped";

    fn into_handle(self) -> Arc<Self> {
        Arc::new(self)
    }
}

/// Boxes a [`Scoped`] as `cx`'s scope stores it, like a generated factory.
fn boxed<const N: usize>(cx: &ComponentConstructionContext) -> BoxedComponent {
    BoxedComponent::stored(
        TypeDescriptor::of::<Scoped<N>>(Scoped::<N>::NAME),
        cx.stored(Scoped::<N>.into_handle()),
    )
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
            scope: None,
        }]
    }
}

fn scoped<const N: usize>(scope: ScopeKey) -> ComponentDescriptor {
    ComponentDescriptor {
        id: Box::leak(format!("contract-scoped-{N}").into_boxed_str()),
        scope,
        factories: factories::<N>,
        ..ComponentDescriptor::of::<Scoped<N>>()
    }
}

/// The descriptors of `Scoped<0>` to `Scoped<$last>` in `$scope`.
macro_rules! roster {
    ($scope:expr; $($n:literal),* $(,)?) => {
        vec![ $( scoped::<$n>($scope) ),* ]
    };
}

/// The first `count` roster components in `scope`, `Scoped<0>` first. Leaked, because an app
/// builder borrows its descriptors for the program's lifetime.
///
/// # Panics
///
/// When `count` exceeds `UNUSED + 1`, the size of the roster.
pub(crate) fn roster(scope: ScopeKey, count: usize) -> &'static [ComponentDescriptor] {
    let roster = roster!(
        scope;
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
        25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46,
        47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64
    );

    Box::leak(roster[..count].to_vec().into_boxed_slice())
}

/// A runtime that runs everything on the calling thread, so the thread-scoped tracking
/// allocator sees every allocation a call makes.
pub(crate) fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("the contract runtime builds")
}

/// What `CALLS` calls of `call` allocate on `runtime`, after unmeasured warm-up calls. Every
/// call must report success, so a contract over failing calls cannot pass.
pub(crate) fn per_call<F: Future<Output = bool>>(
    runtime: &tokio::runtime::Runtime,
    call: impl Fn() -> F,
) -> Delta {
    for _ in 0..WARMUP {
        assert!(runtime.block_on(call()), "a warm-up call failed");
    }

    let (succeeded, delta) = measure(|| (0..CALLS).filter(|_| runtime.block_on(call())).count());

    assert_eq!(succeeded, CALLS, "a measured call failed");

    delta
}
