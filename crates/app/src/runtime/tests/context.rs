//! Invocation contexts established on a runtime and over the scopes its openers open.

use upwell_di::context::{Invocation, InvocationContext};

use super::*;
use crate::{EntryDescriptor, EntryScope, ScopeOpener};

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
    session.establish(sessions(runtime).context(Invocation::RpcCall))
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
        first.context(Invocation::Hook { kind: "startup" }),
        second.context(Invocation::Hook { kind: "startup" }),
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
    let context = runtime.context(Invocation::Hook { kind: "reload" });

    publish_next(&runtime).await;

    assert!(context.resolves_root(&pinned));
    assert!(!context.resolves_root(&runtime.root()));
}

#[tokio::test]
async fn an_opener_context_without_a_scope_follows_the_current_generation() {
    let runtime = runtime().await;
    let context = sessions(&runtime).context(Invocation::HttpRequest);

    publish_next(&runtime).await;

    assert!(context.resolves_root(&runtime.root()));
}

#[tokio::test]
async fn a_context_in_a_scope_keeps_the_generation_the_scope_pinned() {
    let runtime = runtime().await;
    let pinned = runtime.root();
    let session = open_session(&runtime).await;
    let context = runtime
        .context(Invocation::RpcCall)
        .with_scope(Arc::clone(session.container()));

    publish_next(&runtime).await;

    assert!(context.resolves_root(&pinned));
    assert!(!context.resolves_root(&runtime.root()));
}

#[tokio::test]
async fn run_work_runs_in_a_context_on_its_instance() {
    let runtime = runtime().await;
    let sessions = sessions(&runtime);

    let (invocation, scope, visitor) = sessions
        .builder(Session)
        .seed(Visitor(7))
        .run(|visitor: Visitor| async move {
            let context = captured();

            (context.invocation().clone(), context.scope_key(), visitor)
        })
        .await
        .expect("the run succeeds");

    assert_eq!(invocation, Invocation::ScopeRun { scope: Session::ID });
    assert_eq!(scope, Some(Session::KEY));
    assert_eq!(visitor, Visitor(7));
    assert!(
        InvocationContext::capture().is_err(),
        "the run's context ends with its work"
    );
}
