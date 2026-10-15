use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::FutureExt;
use futures::stream::{self, FuturesUnordered, Stream, StreamExt};
use upwell_core::ResolverCtx;

use super::FiredHook;
use crate::{Error, HookDescriptor, HookKind, HookParticipation, HookRole, HookRouting, Result};

type Ctx = Option<Arc<dyn ResolverCtx + Send + Sync>>;

/// Streams one event's outcomes, each tagged with its hook's registration index.
///
/// The selection is applied eagerly, so the stream borrows only `bucket` and `cx`.
/// Participants run one at a time in registration order and the stream stops after the
/// first outcome that ends the event. Otherwise every selected observer then starts and is
/// yielded in completion order.
pub(super) fn fire_indexed<'a, K: HookKind>(
    bucket: &'a [HookDescriptor],
    ctx: Ctx,
    cx: &'a K::Cx,
    select: impl Fn(&HookDescriptor) -> bool,
) -> impl Stream<Item = (usize, FiredHook<K>)> + 'a {
    let ended = Arc::new(AtomicBool::new(false));
    let (participants, observers): (Vec<_>, Vec<_>) = bucket
        .iter()
        .enumerate()
        .filter(|(_, hook)| hook.routing == HookRouting::Always || select(hook))
        .partition(|(_, hook)| role_of::<K>(hook) == HookRole::Participant);

    participants_stream::<K>(participants, ctx.clone(), cx, Arc::clone(&ended))
        .chain(observers_stream::<K>(observers, ctx, cx, ended))
}

fn participants_stream<'a, K: HookKind>(
    participants: Vec<(usize, &'a HookDescriptor)>,
    ctx: Ctx,
    cx: &'a K::Cx,
    ended: Arc<AtomicBool>,
) -> impl Stream<Item = (usize, FiredHook<K>)> + 'a {
    stream::unfold(
        (participants.into_iter(), ctx, ended),
        move |(mut participants, ctx, ended)| async move {
            if ended.load(Ordering::Acquire) {
                return None;
            }

            let (index, hook) = participants.next()?;
            let outcome = invoke::<K>(hook, ctx.as_deref(), cx)
                .await
                .and_then(|boxed| downcast_output::<K>(hook, boxed));

            if K::ends_event(&outcome) {
                ended.store(true, Ordering::Release);
            }

            let fired = FiredHook::Participant {
                component: hook.component_ty,
                outcome,
            };

            Some(((index, fired), (participants, ctx, ended)))
        },
    )
}

/// Starts every observer once the participants are exhausted, unless one ended the event.
fn observers_stream<'a, K: HookKind>(
    observers: Vec<(usize, &'a HookDescriptor)>,
    ctx: Ctx,
    cx: &'a K::Cx,
    ended: Arc<AtomicBool>,
) -> impl Stream<Item = (usize, FiredHook<K>)> + 'a {
    stream::once(async move {
        if ended.load(Ordering::Acquire) {
            return FuturesUnordered::new();
        }

        observers
            .into_iter()
            .map(|(index, hook)| {
                let ctx = ctx.clone();

                async move {
                    let outcome = invoke::<K>(hook, ctx.as_deref(), cx).await.map(drop);
                    let fired = FiredHook::Observer {
                        component: hook.component_ty,
                        outcome,
                    };

                    (index, fired)
                }
            })
            .collect()
    })
    .flatten()
}

/// The effective role of `hook` under kind `K`: every hook of an observer-only kind is an
/// observer, whatever its descriptor declares.
pub(super) fn role_of<K: HookKind>(hook: &HookDescriptor) -> HookRole {
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
