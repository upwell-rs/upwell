use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, RwLock, Weak};

use futures::FutureExt;
use upwell_core::{ResolverCtx, TypeDescriptor};

use crate::{Error, HookDescriptor, HookKind, HookParticipation, HookRole, HookRouting, Result};

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

/// What one [`HookManager::fire`] ran: every participant outcome in registration order,
/// every observer outcome in registration order, and whether a participant ended the
/// event.
#[must_use = "observer and participant failures are reported only through the fired outcomes"]
#[non_exhaustive]
pub struct FiredHooks<K: HookKind> {
    /// Each participant that ran and its outcome. The last one ended the event when
    /// [`ended`](Self::ended) is set.
    pub participants: Vec<(TypeDescriptor, Result<K::Output>)>,
    /// Each observer that ran and whether it completed. Empty when the event ended.
    pub observers: Vec<(TypeDescriptor, Result<()>)>,
    /// Whether a participant's outcome ended the event, per
    /// [`HookKind::ends_event`].
    pub ended: bool,
}

impl<K: HookKind> FiredHooks<K> {
    fn empty() -> Self {
        Self {
            participants: Vec::new(),
            observers: Vec::new(),
            ended: false,
        }
    }
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

    /// Attaches the resolver context hook receivers resolve through.
    ///
    /// Attaching replaces any previously attached provider, so a second attach retargets
    /// the manager. The manager retains only a weak reference, so a component storing its
    /// own manager cannot create a root-container cycle; once the attached context is
    /// dropped, hooks report [`Error::ResolverUnavailable`] instead of resolving.
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
                        && role_of::<K>(hook) == HookRole::Participant
                })
            })
    }

    /// Fires an event of kind `K` against `cx`.
    ///
    /// A hook runs when `select` includes it or its routing is
    /// [`Always`](HookRouting::Always). Participants run first, sequentially in
    /// registration order, and the first outcome for which [`HookKind::ends_event`] holds
    /// ends the event: no later participant and no observer runs. Otherwise every
    /// selected observer then runs concurrently, after the event has settled, and its
    /// failure is only reported. Panics are isolated into redacted
    /// [`Error::Panicked`] outcomes. Returns empty outcomes (no work) when nothing
    /// listens.
    pub async fn fire<K: HookKind>(
        &self,
        cx: &K::Cx,
        select: impl Fn(&HookDescriptor) -> bool,
    ) -> FiredHooks<K> {
        let mut fired = FiredHooks::empty();
        let Some(bucket) = self.inner.by_kind.get(&TypeId::of::<K>()) else {
            return fired;
        };
        let ctx = self.resolver_context();
        let selected = |hook: &&HookDescriptor| hook.routing == HookRouting::Always || select(hook);

        for hook in bucket
            .iter()
            .filter(selected)
            .filter(|hook| role_of::<K>(hook) == HookRole::Participant)
        {
            let outcome = invoke::<K>(hook, ctx.as_deref(), cx)
                .await
                .and_then(|boxed| downcast_output::<K>(hook, boxed));
            let ended = K::ends_event(&outcome);

            fired.participants.push((hook.component_ty, outcome));

            if ended {
                fired.ended = true;

                return fired;
            }
        }

        let observers = bucket
            .iter()
            .filter(selected)
            .filter(|hook| role_of::<K>(hook) == HookRole::Observer)
            .map(|hook| {
                let ctx = ctx.clone();

                async move {
                    let outcome = invoke::<K>(hook, ctx.as_deref(), cx).await.map(drop);

                    (hook.component_ty, outcome)
                }
            });

        fired.observers = futures::future::join_all(observers).await;

        fired
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

/// The effective role of `hook` under kind `K`: every hook of an observer-only kind is an
/// observer, whatever its descriptor declares.
fn role_of<K: HookKind>(hook: &HookDescriptor) -> HookRole {
    match K::PARTICIPATION {
        HookParticipation::ObserversOnly => HookRole::Observer,
        HookParticipation::ParticipantsAndObservers => hook.role,
    }
}

/// Runs one hook's erased call inside a panic boundary, or reports
/// [`Error::ResolverUnavailable`] when no resolver context is attached.
async fn invoke<K: HookKind>(
    hook: &HookDescriptor,
    ctx: Option<&(dyn ResolverCtx + Send + Sync)>,
    cx: &K::Cx,
) -> Result<Box<dyn Any + Send>> {
    let panicked = || Error::Panicked {
        hook: K::NAME,
        component: hook.component_ty.name,
    };
    let Some(ctx) = ctx else {
        return Err(Error::ResolverUnavailable);
    };
    let future = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        (hook.call)(ctx, cx as &(dyn Any + Send + Sync))
    }))
    .map_err(|_| panicked())?;

    std::panic::AssertUnwindSafe(future)
        .catch_unwind()
        .await
        .map_err(|_| panicked())?
}

fn downcast_output<K: HookKind>(
    hook: &HookDescriptor,
    boxed: Box<dyn Any + Send>,
) -> Result<K::Output> {
    boxed
        .downcast::<K::Output>()
        .map(|boxed| *boxed)
        .map_err(|_| Error::InvalidOutput {
            hook: K::NAME,
            component: hook.component_ty.name,
        })
}
