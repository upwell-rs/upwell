//! Whether an opened instance holds an end: only when a capture sink or an end hook that
//! applies to its scope exists.

use std::future::Future;
use std::pin::Pin;

use upwell_core::TypeDescriptor;
use upwell_hooks::{HookDescriptor, HookRole, Result as HookResult};

use super::*;
use crate::{AnyScopeEnded, ScopeEnded};

struct Observer;

fn observe<'a>(
    _ctx: &'a (dyn upwell_core::ResolverCtx + Send + Sync),
    _cx: &'a (dyn std::any::Any + Send + Sync),
) -> BoxedHook<'a> {
    Box::pin(async { Ok(Box::new(()) as Box<dyn std::any::Any + Send>) })
}

type BoxedHook<'a> =
    Pin<Box<dyn Future<Output = HookResult<Box<dyn std::any::Any + Send>>> + Send + 'a>>;

fn hook<K: upwell_hooks::HookKind>() -> HookDescriptor {
    HookDescriptor::of::<K>(0, TypeDescriptor::of::<Observer>("Observer"), observe)
        .with_role(HookRole::Observer)
}

async fn opens_with_end(hooks: Vec<HookDescriptor>) -> (bool, bool) {
    let (runtime, _registry) = build_runtime_with_hooks(HashMap::new(), &[], hooks).await;
    let session = open_session(&runtime).await;
    let request = runtime
        .opener::<Request>()
        .expect("request is declared")
        .builder(Request)
        .under(&session)
        .open_default()
        .await
        .expect("request opens");

    (session.has_end(), request.has_end())
}

#[tokio::test]
async fn an_instance_nothing_observes_holds_no_end() {
    assert_eq!(opens_with_end(Vec::new()).await, (false, false));
}

#[tokio::test]
async fn a_typed_hook_gives_an_end_to_its_scope_only() {
    let hooks = vec![hook::<ScopeEnded<Request>>()];

    assert_eq!(opens_with_end(hooks).await, (false, true));
}

#[tokio::test]
async fn an_any_scope_hook_gives_an_end_to_every_scope() {
    let hooks = vec![hook::<AnyScopeEnded>()];

    assert_eq!(opens_with_end(hooks).await, (true, true));
}
