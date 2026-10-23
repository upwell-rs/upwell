use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use upwell_core::{ResolverCtx, ResolverCtxExt};
use upwell_di::{Component, ComponentSource};

/// The boxed future a hand-written [`HookCall`](upwell_hooks::HookCall) returns.
pub(crate) type HookFuture<'a> =
    Pin<Box<dyn Future<Output = upwell_hooks::Result<Box<dyn Any + Send>>> + Send + 'a>>;

/// Resolves a hook's `&self` receiver `C` through the firing manager's resolver context,
/// the way `#[hook]` generates it: from the root that manager is attached to, so a run
/// against a stale manager cannot reach the current instance.
pub(crate) fn hook_receiver<C>(
    ctx: &(dyn ResolverCtx + Send + Sync),
) -> upwell_hooks::Result<Arc<C>>
where
    C: Component<Handle = Arc<C>>,
{
    ctx.get_resolver::<ComponentSource>()
        .and_then(|source| source.component::<C>())
        .ok_or(upwell_hooks::Error::MissingReceiver(C::NAME))
}

/// The call of a hook that planning and projection tests register but never fire.
pub(crate) fn uncalled_hook<'a>(
    _: &'a (dyn ResolverCtx + Send + Sync),
    _: &'a (dyn Any + Send + Sync),
) -> HookFuture<'a> {
    panic!("a planning test fired a hook")
}

/// Declares a hand-written [`HookCall`](upwell_hooks::HookCall) `$name` that binds the
/// resolver context and the hook context to the closure's two parameters and boxes the
/// future its body returns.
macro_rules! hook_call {
    ($name:ident, |$ctx:pat_param, $cx:pat_param| $body:expr) => {
        fn $name<'a>(
            $ctx: &'a (dyn ::upwell_core::ResolverCtx + Send + Sync),
            $cx: &'a (dyn ::std::any::Any + Send + Sync),
        ) -> $crate::test_support::HookFuture<'a> {
            ::std::boxed::Box::pin($body)
        }
    };
}

pub(crate) use hook_call;
