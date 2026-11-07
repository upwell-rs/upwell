//! Scope logs deciding capture by the invocation context, for work carried between tasks,
//! threads and pools.

#![cfg(feature = "tracing-subscriber")]

use rayon::prelude::*;
use tracing::{Dispatch, Instrument};
use tracing_subscriber::Registry;
use tracing_subscriber::layer::SubscriberExt;
use upwell_di::context::rayon::ParallelIteratorExt;
use upwell_di::context::{self, CapturedContext, InvocationContext, InvocationTarget, TargetRef};

use super::logs::{install, messages};
use super::*;
use crate::logs::{InstanceLogs, LogOptions, ScopeLogLayer, ScopeLogs};

/// A target standing in for a protocol's call.
#[derive(Debug)]
struct Call;

impl InvocationTarget for Call {}

fn sessions(runtime: &AppRuntime) -> ScopeOpener<Session> {
    runtime.opener::<Session>().expect("session is declared")
}

/// The context of a call running in `scope`.
fn call_in<S: Scope>(runtime: &AppRuntime, scope: &OpenScope<S>) -> InvocationContext {
    sessions(runtime)
        .context(TargetRef::from_static(&Call))
        .with_scope(Arc::clone(scope.container()))
}

async fn capturing_session(runtime: &AppRuntime) -> OpenScope<Session> {
    sessions(runtime)
        .builder(Session)
        .capture_logs()
        .open_on_demand()
        .await
        .expect("the session opens")
}

/// Opens a capturing instance on this thread, logs `message` inside its span, and returns
/// whether `ScopeLogs` saw it capture and what it captured.
fn capture_on_this_thread(dispatch: &Dispatch, message: &'static str) -> (bool, Vec<String>) {
    tracing::dispatcher::with_default(dispatch, || {
        let options = LogOptions {
            capture: true,
            ..LogOptions::new()
        };
        let logs = InstanceLogs::open(&Session, &options, None);
        let stream = logs.sink().expect("the instance captures").subscribe();
        let capturing = logs.span().in_scope(|| {
            tracing::info!("{message}");

            ScopeLogs::current().is_capturing()
        });

        logs.finish();

        (capturing, futures::executor::block_on(messages(stream)))
    })
}

#[tokio::test]
async fn a_rayon_job_captured_outside_every_invocation_never_logs_into_a_waiting_capture() {
    let _guard = install();
    let (runtime, _registry) = build_runtime(HashMap::new()).await;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .expect("a rayon pool starts");
    let unbound = (0..1).into_par_iter().current();
    let opener = sessions(&runtime);
    let run = opener
        .builder(Session)
        .capture_logs()
        .run(move || async move {
            let captured = CapturedContext::capture().expect("the run is an invocation");
            let dispatch = tracing::dispatcher::get_default(Dispatch::clone);

            tracing::info!("kept");
            captured.run(|| {
                context::rayon::install(&pool, || {
                    tracing::dispatcher::with_default(&dispatch, || {
                        unbound.for_each(|_| tracing::info!("leaked"));
                    });
                });
            });
        });
    let stream = run.logs();

    run.await.expect("the run succeeds");

    assert_eq!(messages(stream).await, ["kept"]);
}

#[tokio::test]
async fn a_tokio_task_spawned_outside_every_invocation_never_logs_into_the_capture_polling_it() {
    let _guard = install();
    let (runtime, _registry) = build_runtime(HashMap::new()).await;
    let session = capturing_session(&runtime).await;
    let stream = session.logs();
    let local = tokio::task::LocalSet::new();
    let leaked = {
        let _entered = local.enter();

        context::tokio::spawn_local(async { tracing::info!("leaked") })
    };

    call_in(&runtime, &session)
        .enter(
            async {
                tracing::info!("kept");
                local
                    .run_until(leaked)
                    .await
                    .expect("the local task completes");
            }
            .instrument(session.span()),
        )
        .await;
    drop(session);

    assert_eq!(messages(stream).await, ["kept"]);
}

#[tokio::test]
async fn a_carried_tokio_task_sees_and_logs_into_the_run_it_was_spawned_in() {
    let _guard = install();
    let (runtime, _registry) = build_runtime(HashMap::new()).await;
    let opener = sessions(&runtime);
    let run = opener.builder(Session).capture_logs().run(|| async {
        context::tokio::spawn(async {
            tracing::info!("child");

            ScopeLogs::current().is_capturing()
        })
        .await
        .expect("the child completes")
    });
    let stream = run.logs();

    let capturing = run.await.expect("the run succeeds");

    assert!(capturing, "the child sees the run's log");
    assert_eq!(messages(stream).await, ["child"]);
}

#[tokio::test]
async fn a_bound_closure_logs_into_its_own_invocation_inside_another() {
    let _guard = install();
    let (runtime, _registry) = build_runtime(HashMap::new()).await;
    let opener = sessions(&runtime);
    let other = opener.clone();
    let first = opener.builder(Session).capture_logs().run(|| async move {
        let bound = CapturedContext::capture()
            .expect("the run is an invocation")
            .bind(|| {
                let seen = ScopeLogs::current().subscribe();

                tracing::info!("from the first");

                seen
            });
        let second = other
            .builder(Session)
            .capture_logs()
            .run(move || async move {
                tracing::info!("in the second");

                bound()
            });
        let second_stream = second.logs();
        let seen = second.await.expect("the second run succeeds");

        (seen, second_stream)
    });
    let first_stream = first.logs();

    let (seen, second_stream) = first.await.expect("the first run succeeds");

    assert_eq!(messages(first_stream).await, ["from the first"]);
    assert_eq!(messages(second_stream).await, ["in the second"]);
    assert_eq!(
        messages(seen).await,
        ["from the first"],
        "ScopeLogs inside the bound closure is the first run's"
    );
}

#[tokio::test]
async fn an_entered_context_routes_past_the_span_of_an_unrelated_invocation() {
    let _guard = install();
    let (runtime, _registry) = build_runtime(HashMap::new()).await;
    let opener = sessions(&runtime);
    let other = opener.clone();
    let first = opener.builder(Session).capture_logs().run(|| async move {
        let context = InvocationContext::capture().expect("the run is an invocation");
        let second = other
            .builder(Session)
            .capture_logs()
            .run(move || async move {
                context.enter_sync(|| tracing::info!("the first's context, the second's span"));
            });
        let second_stream = second.logs();

        second.await.expect("the second run succeeds");

        second_stream
    });
    let first_stream = first.logs();

    let second_stream = first.await.expect("the first run succeeds");

    assert_eq!(
        messages(first_stream).await,
        ["the first's context, the second's span"]
    );
    assert!(messages(second_stream).await.is_empty());
}

#[test]
fn masked_work_never_captures_while_work_without_any_context_keeps_span_routing() {
    let dispatch = Dispatch::new(Registry::default().with(ScopeLogLayer::new()));
    let absent = {
        let dispatch = dispatch.clone();

        std::thread::spawn(move || capture_on_this_thread(&dispatch, "absent"))
    };
    let masked = context::thread::spawn(move || capture_on_this_thread(&dispatch, "masked"));

    let absent = absent.join().expect("the plain thread completes");
    let masked = masked.join().expect("the carried thread completes");

    assert_eq!(absent, (true, vec!["absent".to_owned()]));
    assert_eq!(masked, (false, Vec::new()));
}

#[tokio::test]
async fn an_instance_opening_under_the_invocation_captures_its_own_events() {
    let _guard = install();
    let (runtime, _registry) = build_runtime(HashMap::new()).await;
    let session = open_session(&runtime).await;
    let requests = runtime.opener::<Request>().expect("request is declared");

    let (capturing, stream) = call_in(&runtime, &session)
        .enter(async {
            let request = requests
                .builder(Request)
                .under(&session)
                .capture_logs()
                .open_on_demand()
                .await
                .expect("the request opens");
            let stream = request.logs();
            let capturing = request.span().in_scope(|| {
                tracing::info!("in the request");

                ScopeLogs::current().is_capturing()
            });

            (capturing, stream)
        })
        .await;

    assert!(capturing, "the request's log is current inside its span");
    assert_eq!(messages(stream).await, ["in the request"]);
}

/// The documented cost of routing by the context: a span entered by hand inside an unrelated
/// invocation logs to that invocation, not to the instance the span belongs to.
#[tokio::test]
async fn a_span_entered_by_hand_in_an_unrelated_invocation_logs_to_the_invocation() {
    let _guard = install();
    let (runtime, _registry) = build_runtime(HashMap::new()).await;
    let opener = sessions(&runtime);
    let other = opener.clone();
    let run = opener.builder(Session).capture_logs().run(|| async move {
        let unrelated = other
            .builder(Session)
            .capture_logs()
            .open_on_demand()
            .await
            .expect("the unrelated session opens");
        let unrelated_stream = unrelated.logs();

        unrelated
            .span()
            .in_scope(|| tracing::info!("entered by hand"));

        unrelated_stream
    });
    let run_stream = run.logs();

    let unrelated_stream = run.await.expect("the run succeeds");

    assert_eq!(messages(run_stream).await, ["entered by hand"]);
    assert!(messages(unrelated_stream).await.is_empty());
}

#[tokio::test]
async fn an_open_instance_inside_a_scopeless_hook_context_captures_by_its_span() {
    let _guard = install();
    let (runtime, _registry) = build_runtime(HashMap::new()).await;
    let hook = sessions(&runtime).context(crate::runtime::hook_target!("startup"));

    let stream = hook
        .enter(async {
            let session = capturing_session(&runtime).await;
            let stream = session.logs();

            session.span().in_scope(|| tracing::info!("in the hook"));

            stream
        })
        .await;

    assert_eq!(messages(stream).await, ["in the hook"]);
}

#[tokio::test]
async fn an_explicit_scope_off_disables_capture_inside_a_run() {
    let layer = crate::builtins::logging::scope_log_layer(
        &crate::builtins::config::LoggingConfig::default(),
        Some(std::ffi::OsStr::new("myapp=info,upwell::scope=off")),
    )
    .expect("filter directives are valid");
    let _guard = tracing::dispatcher::set_default(&Dispatch::new(Registry::default().with(layer)));
    let (runtime, _registry) = build_runtime(HashMap::new()).await;
    let opener = sessions(&runtime);
    let run = opener.builder(Session).capture_logs().run(|| async {
        tracing::info!(target: "myapp", "inside the run");

        ScopeLogs::current().is_capturing()
    });
    let stream = run.logs();

    let capturing = run.await.expect("the run succeeds");

    assert!(
        !capturing,
        "ScopeLogs reports no capture under upwell::scope=off"
    );
    assert!(messages(stream).await.is_empty());
}
