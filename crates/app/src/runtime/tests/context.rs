//! Invocation contexts established on a runtime and over the scopes its openers open.

use upwell_di::context::{InvocationContext, InvocationTarget, TargetRef};

use super::*;
use crate::runtime::{hook_target, scope_run_target};
use crate::{EntryDescriptor, EntryScope, HookTarget, ScopeOpener, ScopeRunTarget};

/// A target standing in for a protocol's call.
#[derive(Debug)]
struct Call;

impl InvocationTarget for Call {}

/// A runtime with an entry injecting nothing (`test/ping`), so its session scope defers.
async fn runtime() -> AppRuntime {
    let entries = [EntryDescriptor::new::<Session, ()>(key("test/ping"))];
    let (runtime, _) = build_runtime_with_entries(visitor_destination(), &entries).await;

    runtime
}

fn sessions(runtime: &AppRuntime) -> ScopeOpener<Session> {
    runtime.opener::<Session>().expect("session is declared")
}

/// The context of an RPC call running in `session`.
fn call_context(runtime: &AppRuntime, session: &EntryScope<Session>) -> InvocationContext {
    session.establish(sessions(runtime).context(TargetRef::from_static(&Call)))
}

async fn deferred_session(runtime: &AppRuntime) -> EntryScope<Session> {
    let sessions = sessions(runtime);
    let entry = sessions
        .entry(key("test/ping"))
        .expect("the entry resolves");

    sessions
        .builder(Session)
        .seed(Visitor(4))
        .open_when_needed(Some(&entry))
        .await
        .expect("an entry planning nothing defers its scope")
}

fn captured() -> InvocationContext {
    InvocationContext::capture().expect("a context is active")
}

#[test]
fn generic_targets_use_static_storage() {
    fn hook<K: upwell_hooks::HookKind>() -> TargetRef {
        hook_target!(K::NAME)
    }

    fn scope<S: upwell_core::Scope>() -> TargetRef {
        scope_run_target!(S::ID)
    }

    let hook = hook::<upwell_hooks::Startup>();
    let scope = scope::<Session>();

    assert!(matches!(hook, TargetRef::Static(_)));
    assert!(matches!(scope, TargetRef::Static(_)));
    assert_eq!(
        hook.downcast_ref::<HookTarget>().expect("hook target").kind,
        "startup"
    );
    assert_eq!(
        scope
            .downcast_ref::<ScopeRunTarget>()
            .expect("scope target")
            .scope,
        Session::ID
    );
}

#[tokio::test]
async fn a_deferred_scope_binds_into_the_context_waiting_for_it() {
    let runtime = runtime().await;
    let session = deferred_session(&runtime).await;
    let context = call_context(&runtime, &session);

    context
        .enter(async {
            assert_eq!(captured().scope_key(), Some(Singleton::KEY));

            let visitor = session
                .extract::<Visitor>()
                .await
                .expect("resolving opens the deferred scope");
            let opened = session.container().await.expect("the scope is open");

            assert_eq!(visitor, Visitor(4));
            assert!(captured().is_scope(opened));
        })
        .await;
}

#[tokio::test]
async fn an_open_scope_is_the_context_scope_at_once() {
    let runtime = runtime().await;
    let session =
        EntryScope::<Session>::from_container(Arc::clone(open_session(&runtime).await.container()));
    let context = call_context(&runtime, &session);

    assert!(context.is_scope(session.current()));
}

#[tokio::test]
async fn contexts_of_two_runtimes_resolve_their_own_roots() {
    let first = runtime().await;
    let second = runtime().await;
    let contexts = [
        first.context(hook_target!("startup")),
        second.context(hook_target!("startup")),
    ];

    let roots = contexts.map(|context| {
        (
            context.resolves_root(&first.root()),
            context.resolves_root(&second.root()),
        )
    });

    assert_eq!(roots, [(true, false), (false, true)]);
}

#[tokio::test]
async fn a_context_without_a_scope_keeps_the_generation_it_was_established_on() {
    let runtime = runtime().await;
    let pinned = runtime.root();
    let context = runtime.context(hook_target!("reload"));

    publish_next(&runtime).await;

    assert!(context.resolves_root(&pinned));
    assert!(!context.resolves_root(&runtime.root()));
}

#[tokio::test]
async fn an_opener_context_without_a_scope_follows_the_current_generation() {
    let runtime = runtime().await;
    let context = sessions(&runtime).context(TargetRef::from_static(&Call));

    publish_next(&runtime).await;

    assert!(context.resolves_root(&runtime.root()));
}

#[tokio::test]
async fn a_context_in_a_scope_keeps_the_generation_the_scope_pinned() {
    let runtime = runtime().await;
    let pinned = runtime.root();
    let session = open_session(&runtime).await;
    let context = runtime
        .context(TargetRef::from_static(&Call))
        .with_scope(Arc::clone(session.container()));

    publish_next(&runtime).await;

    assert!(context.resolves_root(&pinned));
    assert!(!context.resolves_root(&runtime.root()));
}

#[tokio::test]
async fn run_work_runs_in_a_context_on_its_instance() {
    let runtime = runtime().await;
    let sessions = sessions(&runtime);

    let (target, scope, visitor) = sessions
        .builder(Session)
        .seed(Visitor(7))
        .run(|visitor: Visitor| async move {
            let context = captured();

            (
                context.target().downcast_ref::<ScopeRunTarget>().copied(),
                context.scope_key(),
                visitor,
            )
        })
        .await
        .expect("the run succeeds");

    assert_eq!(target, Some(ScopeRunTarget { scope: Session::ID }));
    assert_eq!(scope, Some(Session::KEY));
    assert_eq!(visitor, Visitor(7));
    assert!(
        InvocationContext::capture().is_err(),
        "the run's context ends with its work"
    );
}
