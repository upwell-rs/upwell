use std::any::TypeId;
use std::collections::HashMap;
use std::sync::{Arc, RwLock, Weak};

use futures::stream::{Stream, StreamExt};
use upwell_core::ResolverCtx;

use crate::{HookDescriptor, HookKind, HookRole};

mod fired;
mod stream;

pub use fired::{FiredHook, FiredHooks};

/// Fires hooks of a kind against the live component instances.
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

type ResolverProvider = dyn Fn() -> Option<Arc<dyn ResolverCtx + Send + Sync>> + Send + Sync;

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

    /// Attaches the resolver context hook receivers resolve through.
    ///
    /// Attaching replaces any previously attached provider, so a second attach retargets
    /// the manager. The manager retains only a weak reference, so a component storing its
    /// own manager cannot create a root-container cycle; once the attached context is
    /// dropped, hooks report
    /// [`Error::ResolverUnavailable`](crate::Error::ResolverUnavailable) instead of
    /// resolving.
    ///
    /// Upwell's runtime never retargets: each runtime generation builds a fresh
    /// generation-local manager and attaches it once to the root scope that seeded it,
    /// so a manager stays bound to its own generation's root.
    pub fn attach(&self, ctx: Weak<dyn ResolverCtx + Send + Sync>) {
        self.attach_resolver_provider(move || ctx.upgrade());
    }

    /// Attaches a provider that selects the resolver context for each fire.
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

    /// Whether any hook of kind `K`, participant or observer, is registered — an O(1)
    /// check a firing site uses to skip building the event entirely when nothing listens.
    pub fn has<K: HookKind>(&self) -> bool {
        self.inner.by_kind.contains_key(&TypeId::of::<K>())
    }

    /// Whether `component` declares a participant hook of kind `K`.
    pub fn component_participates<K: HookKind>(&self, component: TypeId) -> bool {
        self.inner
            .by_kind
            .get(&TypeId::of::<K>())
            .is_some_and(|hooks| {
                hooks.iter().any(|hook| {
                    hook.component_ty.type_id == component
                        && stream::role_of::<K>(hook) == HookRole::Participant
                })
            })
    }

    /// Fires an event of kind `K` against `cx` and collects every outcome.
    ///
    /// A hook runs when `select` includes it or its routing is
    /// [`Always`](crate::HookRouting::Always). Participants run first, sequentially in
    /// registration order, and the first outcome for which [`HookKind::ends_event`] holds
    /// ends the event: no later participant and no observer runs. Otherwise every
    /// selected observer then runs concurrently, after the event has settled, and its
    /// failure is only reported. Observers are collected back into registration order.
    /// Panics are isolated into redacted [`Error::Panicked`](crate::Error::Panicked)
    /// outcomes. Returns empty outcomes (no work) when nothing listens.
    pub async fn fire<K: HookKind>(
        &self,
        cx: &K::Cx,
        select: impl Fn(&HookDescriptor) -> bool,
    ) -> FiredHooks<K> {
        let fired = self.fire_indexed::<K>(cx, select).collect().await;

        FiredHooks::collect(fired)
    }

    /// Fires an event of kind `K` against every registered hook of that kind, ignoring
    /// targeting, and collects every outcome. See [`fire`](Self::fire).
    pub async fn fire_all<K: HookKind>(&self, cx: &K::Cx) -> FiredHooks<K> {
        self.fire::<K>(cx, |_| true).await
    }

    /// Fires an event of kind `K` against `cx` and yields each outcome as soon as its
    /// hook completes.
    ///
    /// Selection, ordering, and the participant/observer split are those of
    /// [`fire`](Self::fire): participants are yielded one at a time in registration
    /// order, the stream ends after a participant that ends the event, and otherwise
    /// every observer starts concurrently and is yielded in completion order. Dropping
    /// the stream cancels the event: hooks that have not started yet never run.
    pub fn fire_streamed<'a, K: HookKind>(
        &'a self,
        cx: &'a K::Cx,
        select: impl Fn(&HookDescriptor) -> bool,
    ) -> impl Stream<Item = FiredHook<K>> + 'a {
        self.fire_indexed::<K>(cx, select).map(|(_, hook)| hook)
    }

    /// Fires an event of kind `K` against every registered hook of that kind, ignoring
    /// targeting, and yields each outcome as it completes. See
    /// [`fire_streamed`](Self::fire_streamed).
    pub fn fire_all_streamed<'a, K: HookKind>(
        &'a self,
        cx: &'a K::Cx,
    ) -> impl Stream<Item = FiredHook<K>> + 'a {
        self.fire_streamed::<K>(cx, |_| true)
    }

    fn fire_indexed<'a, K: HookKind>(
        &'a self,
        cx: &'a K::Cx,
        select: impl Fn(&HookDescriptor) -> bool,
    ) -> impl Stream<Item = (usize, FiredHook<K>)> + 'a {
        let bucket = self
            .inner
            .by_kind
            .get(&TypeId::of::<K>())
            .map_or(&[][..], Vec::as_slice);
        let ctx = if bucket.is_empty() {
            None
        } else {
            self.resolver_context()
        };

        stream::fire_indexed::<K>(bucket, ctx, cx, select)
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
