//! How a request opened under a session, and the session itself, end: the request when its
//! last handle drops after the response, the session only when the app's cache lets it go.

use std::time::Duration;

use upwell::{AnyScopeEnded, RunError, Scope, ScopeEnd, ScopeEndKind, ScopeEnded};

use super::*;

/// Every request scope end the recorder saw.
static REQUEST_ENDS: Mutex<Vec<ScopeEnd>> = Mutex::new(Vec::new());

/// Every session end any-scope observation saw.
static SESSION_ENDS: Mutex<Vec<ScopeEnd>> = Mutex::new(Vec::new());

/// Observes request ends by type and every scope's end, keeping the sessions'.
#[component]
struct EndRecorder {}

#[methods]
impl EndRecorder {
    #[hook(ScopeEnded<HttpRequest>)]
    async fn request_ended(&self, end: ScopeEnd) {
        REQUEST_ENDS
            .lock()
            .expect("the request ends lock is healthy")
            .push(end);
    }

    #[hook(AnyScopeEnded)]
    async fn any_ended(&self, end: ScopeEnd) {
        if end.scope == Session::ID {
            SESSION_ENDS
                .lock()
                .expect("the session ends lock is healthy")
                .push(end);
        }
    }
}

/// The ends in `ends` once `count` of them arrived, awaited while the observers run on their own
/// task.
async fn ends_reaching(ends: &Mutex<Vec<ScopeEnd>>, count: usize) -> Vec<ScopeEnd> {
    for _ in 0..2500 {
        let seen = ends.lock().expect("the ends lock is healthy").clone();

        if seen.len() >= count {
            return seen;
        }

        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    panic!("fewer than {count} scope ends arrived");
}

#[tokio::test]
async fn a_request_under_a_session_ends_once_and_the_session_only_when_released() {
    let dir = config_dir(1);
    let (router, runtime) = build(with_sessions(dir.path()).component::<EndRecorder>()).await;
    let sessions = runtime
        .root()
        .get::<Sessions>()
        .expect("the session cache is a singleton");

    assert_eq!(get(&router, "/session/view", Some("lee")).await, "lee@1");

    let requests = ends_reaching(&REQUEST_ENDS, 1).await;

    assert!(
        runtime.drain_scope_ends(Duration::from_secs(5)).await,
        "every pending end hook ran"
    );
    assert_eq!(requests.len(), 1, "the request ended once");
    assert_eq!(requests[0].kind, ScopeEndKind::Released);
    assert!(
        SESSION_ENDS
            .lock()
            .expect("the session ends lock is healthy")
            .is_empty(),
        "the cached session outlives its request"
    );

    sessions.forget("lee");

    let ended = ends_reaching(&SESSION_ENDS, 1).await;

    assert_eq!(ended.len(), 1, "the session ended once");
    assert_eq!(ended[0].kind, ScopeEndKind::Released);
    assert_eq!(
        REQUEST_ENDS
            .lock()
            .expect("the request ends lock is healthy")
            .len(),
        1,
        "releasing the session ends no request again"
    );
}

#[tokio::test]
async fn run_and_try_run_open_under_a_chosen_parent_and_end_as_their_work_did() {
    let dir = config_dir(1);
    let (_router, runtime) = build(with_sessions(dir.path()).component::<EndRecorder>()).await;
    let sessions = runtime
        .root()
        .get::<Sessions>()
        .expect("the session cache is a singleton");
    let requests = runtime
        .opener::<HttpRequest>()
        .expect("the protocol declares the request scope");
    let parent = RequestParent::new(&sessions.session("mo").await);

    requests
        .builder(HttpRequest)
        .under_parent(&parent)
        .run(|view: Arc<RequestView>| async move {
            assert_eq!(view.describe(), "mo@1", "the work runs under the session");
        })
        .await
        .expect("the run completes");

    let described = requests
        .builder(HttpRequest)
        .under_parent(&parent)
        .try_run(|view: Arc<RequestView>| async move { Ok::<_, std::io::Error>(view.describe()) })
        .await
        .expect("the work succeeds");

    assert_eq!(described, "mo@1");

    let failed = requests
        .builder(HttpRequest)
        .under_parent(&parent)
        .try_run(|_view: Arc<RequestView>| async move {
            Err::<(), _>(std::io::Error::other("the work fails"))
        })
        .await;

    assert!(matches!(failed, Err(RunError::Work(_))));

    let ends = ends_reaching(&REQUEST_ENDS, 3).await;
    let completed = ends
        .iter()
        .filter(|end| end.kind == ScopeEndKind::Completed)
        .count();
    let errored = ends
        .iter()
        .filter(|end| matches!(end.kind, ScopeEndKind::Errored { .. }))
        .count();

    assert_eq!(ends.len(), 3, "each run ended its instance once");
    assert_eq!((completed, errored), (2, 1));
}
