//! A general lifecycle/event **hook** system.
//!
//! A hook is an `async` method on a component or service, marked `#[hook(Kind)]`, that the
//! framework calls when an event of `Kind` occurs. `Kind` is a [`HookKind`] type that owns
//! the hook's *output* (what the method returns) and its *context* (the typed inputs an
//! invocation carries). Built-in kinds are [`Startup`] and [`Shutdown`]; config reload is a
//! kind defined in `upwell-config`. The system is deliberately general so user-defined
//! event kinds can be added the same way.
//!
//! Hooks do **not** receive component dependencies as parameters — those are reached through
//! `&self` (a hook may also be self-less). A hook's only parameters are the kind's inputs,
//! each a [`HookParam`] extracted from the kind's context. Hooks are collected per type
//! (the `{Type}Hooks` distributed slice, exposed via [`ComponentHooks`]) and registered into
//! a [`HookManager`]; a type with no hooks contributes nothing at runtime.
//!
//! This crate is generic over the [`ResolverCtx`](upwell_core::ResolverCtx): a hook's
//! erased [`HookCall`] resolves its receiver through the resolver context, so the hook
//! layer never names the DI container (which sits above it).

mod error;
mod lifecycle;

pub use error::{Error, Result};
pub use lifecycle::{Shutdown, Startup};

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock, Weak};

use futures::FutureExt;
use upwell_core::{DependencyDescriptor, ResolverCtx, TypeDescriptor, UpwellDescriptor};

/// A kind of hook: the event a `#[hook(Kind)]` method reacts to.
///
/// The kind owns the contract: `Output` is what each hook of this kind returns (the kind
/// "decides what output it needs"), and `Cx` is the owned, per-invocation context its
/// parameters are extracted from (e.g. the proposed config values for a reload).
pub trait HookKind: 'static {
    /// What each hook of this kind returns and the runner collects.
    type Output: Send + 'static;

    /// The owned context one invocation carries, that this kind's [`HookParam`]s read.
    /// `Send + Sync` so the hook future (which borrows it) stays `Send`.
    type Cx: Send + Sync + 'static;

    /// A stable name for diagnostics and indexing.
    const NAME: &'static str;
}

/// A parameter a `#[hook(K)]` method may take: an input of kind `K`, extracted from the
/// kind's context — never a component dependency (those come through `&self`).
pub trait HookParam<K: HookKind>: Sized {
    /// The dependency edge this parameter contributes, for validation and event routing
    /// (e.g. the config path a reload hook targets). `path` is the param's `#[config("..")]`
    /// literal, or `None` for the by-type shorthand.
    fn dependency(path: Option<&'static str>) -> DependencyDescriptor;

    /// Extracts this parameter from the kind's context.
    fn extract(cx: &K::Cx, path: Option<&'static str>) -> Result<Self>;
}

/// The erased call shape every hook is compiled to: resolve the receiver (if the method
/// takes `&self`) from the resolver context, extract the kind's params from the erased
/// context, run the method, and box its output. The boxed value is always the kind's
/// `Output`.
///
/// The call takes `&dyn ResolverCtx` (not the container itself), so the hook layer stays
/// below the DI engine. The macro-generated body resolves the receiver through the
/// component source it fetches from the context.
pub type HookCall =
    for<'a> fn(
        &'a (dyn ResolverCtx + Send + Sync),
        &'a (dyn Any + Send + Sync),
    ) -> Pin<Box<dyn Future<Output = Result<Box<dyn Any + Send>>> + Send + 'a>>;

/// Static metadata for one hook, registered into its type's registration set.
///
/// `#[non_exhaustive]`: construct with [`HookDescriptor::new`], never a struct literal, so fields
/// (like [`ordinal`](Self::ordinal)) can be added without breaking downstream construction. The
/// macros and any hand-written descriptor site call `new`.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct HookDescriptor {
    /// Source-position ordinal (the hook method's line), used to run a type's hooks in a stable
    /// source order regardless of registration backend. `linkme` preserves source order via link
    /// order; `inventory`'s linked list does not, so the accessor sorts by this.
    pub ordinal: u32,
    /// The component/service the hook is defined on.
    pub component_ty: TypeDescriptor,
    /// The hook kind's [`NAME`](HookKind::NAME), for diagnostics.
    pub kind: &'static str,
    /// `TypeId::of::<Kind>()`, used to select hooks of a given kind at runtime.
    pub kind_ty: fn() -> TypeId,
    /// The hook's parameter edges (kind inputs), reported at runtime for validation and
    /// event routing.
    pub dependencies: fn() -> Vec<DependencyDescriptor>,
    /// The erased call.
    pub call: HookCall,
}

impl HookDescriptor {
    /// Builds a hook descriptor. The canonical constructor — required because the type is
    /// `#[non_exhaustive]` — so macro-generated and hand-written registration sites survive future
    /// field additions. `const`, so it is usable in the `static`s the `linkme` backend emits.
    pub const fn new(
        ordinal: u32,
        component_ty: TypeDescriptor,
        kind: &'static str,
        kind_ty: fn() -> TypeId,
        dependencies: fn() -> Vec<DependencyDescriptor>,
        call: HookCall,
    ) -> Self {
        Self {
            ordinal,
            component_ty,
            kind,
            kind_ty,
            dependencies,
            call,
        }
    }
}

impl UpwellDescriptor for HookDescriptor {}

impl fmt::Debug for HookDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HookDescriptor")
            .field("component_ty", &self.component_ty)
            .field("kind", &self.kind)
            .field("dependencies", &(self.dependencies)())
            .finish_non_exhaustive()
    }
}

/// A component type's own hooks.
///
/// Implemented for each `#[component]`/`#[service]` by the macro to return that type's
/// `{Type}Hooks` distributed slice — the slice every `#[hook]` method appends to. The owning
/// `ComponentDescriptor` stores this as a fn pointer, so the registry reaches a type's hooks
/// without holding its type.
pub trait ComponentHooks {
    /// Every hook contributed to this type.
    fn hooks() -> &'static [HookDescriptor];
}

/// The empty hooks slice for a type that declares none — the default carried by a
/// `ComponentDescriptor`.
pub fn no_hooks() -> &'static [HookDescriptor] {
    &[]
}

/// Runs hooks of a kind against the live component instances.
///
/// Built once at application build from every registered component's hook slice (empties
/// skipped), so it can be seeded as a framework singleton. The resolver context is attached
/// after the root scope is built (see [`attach`](Self::attach)).
#[derive(Clone)]
pub struct HookManager {
    inner: Arc<HookManagerInner>,
}

struct HookManagerInner {
    resolver: RwLock<Option<Arc<ResolverProvider>>>,
    /// Hooks indexed by kind `TypeId`, so a kind with no listeners is an O(1) miss and a
    /// fire over it does no work at all.
    by_kind: HashMap<TypeId, Vec<HookDescriptor>>,
}

impl HookManager {
    /// Builds a manager over every registered hook (across all component types), grouped by
    /// kind for O(1) listener lookup.
    pub fn new(hooks: Vec<HookDescriptor>) -> Self {
        let mut by_kind: HashMap<TypeId, Vec<HookDescriptor>> = HashMap::new();

        for hook in hooks {
            by_kind.entry((hook.kind_ty)()).or_default().push(hook);
        }

        Self {
            inner: Arc::new(HookManagerInner {
                resolver: RwLock::new(None),
                by_kind,
            }),
        }
    }

    /// Attaches the resolver context owned by the active runtime generation.
    ///
    /// The manager retains only this weak reference so a component storing its own manager cannot
    /// create a root-container cycle. A later runtime generation replaces the previous context.
    pub fn attach(&self, ctx: Weak<dyn ResolverCtx + Send + Sync>) {
        self.attach_resolver_provider(move || ctx.upgrade());
    }

    /// Attaches a provider that selects the resolver context for each hook run.
    ///
    /// Runtime generation owners use this to load the resolver from the same atomic snapshot as
    /// the rest of the committed runtime state.
    #[doc(hidden)]
    pub fn attach_resolver_provider(
        &self,
        provider: impl Fn() -> Option<Arc<dyn ResolverCtx + Send + Sync>> + Send + Sync + 'static,
    ) {
        *self
            .inner
            .resolver
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(provider));
    }

    /// Whether any hook of kind `K` is registered — an O(1) check a firing site uses to
    /// skip building the event entirely when nothing listens.
    pub fn has<K: HookKind>(&self) -> bool {
        self.inner.by_kind.contains_key(&TypeId::of::<K>())
    }

    /// Runs every hook of kind `K` for which `filter` returns true, against `cx`,
    /// **concurrently**, and collects each component's typed outcome (or error) in
    /// registration order. Returns an empty `Vec` (no work) when nothing listens.
    pub async fn run<K: HookKind>(
        &self,
        cx: &K::Cx,
        filter: impl Fn(&HookDescriptor) -> bool,
    ) -> Vec<(TypeDescriptor, Result<K::Output>)> {
        let Some(bucket) = self.inner.by_kind.get(&TypeId::of::<K>()) else {
            return Vec::new();
        };

        let Some(ctx) = self.resolver_context() else {
            return unavailable_outcomes::<K>(bucket, filter);
        };

        let calls = bucket.iter().filter(|hook| filter(hook)).map(|hook| {
            let component = hook.component_ty;
            let ctx = Arc::clone(&ctx);

            async move {
                let outcome = invoke_hook::<K>(hook, ctx.as_ref(), cx).await;

                (component, outcome)
            }
        });

        futures::future::join_all(calls).await
    }

    /// Runs matching hooks sequentially in registration order and stops after the
    /// first failure. Lifecycle startup uses this to guarantee that components after
    /// a failed startup hook never begin side effects.
    pub async fn run_until_error<K: HookKind>(
        &self,
        cx: &K::Cx,
        filter: impl Fn(&HookDescriptor) -> bool,
    ) -> Vec<(TypeDescriptor, Result<K::Output>)> {
        let Some(bucket) = self.inner.by_kind.get(&TypeId::of::<K>()) else {
            return Vec::new();
        };

        let Some(ctx) = self.resolver_context() else {
            return bucket
                .iter()
                .find(|hook| filter(hook))
                .map(|hook| vec![(hook.component_ty, Err(Error::ResolverUnavailable))])
                .unwrap_or_default();
        };
        let mut outcomes = Vec::new();

        for hook in bucket.iter().filter(|hook| filter(hook)) {
            let component = hook.component_ty;
            let outcome = invoke_hook::<K>(hook, ctx.as_ref(), cx).await;
            let failed = outcome.is_err();

            outcomes.push((component, outcome));

            if failed {
                break;
            }
        }

        outcomes
    }

    /// Whether `component` declares a hook of kind `K`.
    pub fn component_has<K: HookKind>(&self, component: TypeId) -> bool {
        self.inner
            .by_kind
            .get(&TypeId::of::<K>())
            .is_some_and(|hooks| {
                hooks
                    .iter()
                    .any(|hook| hook.component_ty.type_id == component)
            })
    }

    fn resolver_context(&self) -> Option<Arc<dyn ResolverCtx + Send + Sync>> {
        let provider = self
            .inner
            .resolver
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()?;

        provider()
    }
}

type ResolverProvider = dyn Fn() -> Option<Arc<dyn ResolverCtx + Send + Sync>> + Send + Sync;

fn unavailable_outcomes<K: HookKind>(
    bucket: &[HookDescriptor],
    filter: impl Fn(&HookDescriptor) -> bool,
) -> Vec<(TypeDescriptor, Result<K::Output>)> {
    bucket
        .iter()
        .filter(|hook| filter(hook))
        .map(|hook| (hook.component_ty, Err(Error::ResolverUnavailable)))
        .collect()
}

async fn invoke_hook<K: HookKind>(
    hook: &HookDescriptor,
    ctx: &(dyn ResolverCtx + Send + Sync),
    cx: &K::Cx,
) -> Result<K::Output> {
    let component = hook.component_ty;
    let future = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        (hook.call)(ctx, cx as &(dyn Any + Send + Sync))
    }))
    .map_err(|_| Error::Panicked {
        hook: K::NAME,
        component: component.name,
    })?;

    std::panic::AssertUnwindSafe(future)
        .catch_unwind()
        .await
        .map_err(|_| Error::Panicked {
            hook: K::NAME,
            component: component.name,
        })?
        .and_then(|boxed| {
            boxed
                .downcast::<K::Output>()
                .map(|boxed| *boxed)
                .map_err(|_| Error::InvalidOutput {
                    hook: K::NAME,
                    component: component.name,
                })
        })
}

/// The stable component id of the seeded [`HookManager`] singleton.
pub const HOOK_MANAGER_ID: &str = "upwell:hook-manager";

/// The display name of the seeded [`HookManager`] singleton.
pub const HOOK_MANAGER_NAME: &str = "HookManager";

#[cfg(test)]
mod tests;
