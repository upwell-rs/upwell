//! Fallible work: how `try_run` reports and ends its instance.

use upwell::{RunError, ScopeEndKind};

use super::*;

#[tokio::test]
async fn work_returning_ok_completes_and_yields_its_value() {
    let app = app("scope-end-try-ok").await;
    let value = opener(&app)
        .builder(ended(50))
        .try_run(|| async { Ok::<_, std::io::Error>(7) })
        .await
        .expect("the work succeeds");

    assert_eq!(value, 7);
    assert_eq!(end_of(50).await.kind, ScopeEndKind::Completed);
}

#[tokio::test]
async fn work_returning_err_errors_with_only_its_type() {
    let app = app("scope-end-try-err").await;
    let error = opener(&app)
        .builder(ended(51))
        .try_run(|| async { Err::<(), _>(std::io::Error::other("hunter2")) })
        .await
        .expect_err("the work fails");

    assert!(matches!(error, RunError::Work(_)));
    assert_eq!(
        end_of(51).await.kind,
        ScopeEndKind::Errored {
            error_type: std::any::type_name::<std::io::Error>()
        }
    );
}

#[tokio::test]
async fn a_failing_constructor_is_a_scope_error() {
    let app = app("scope-end-try-scope").await;
    let failing = app
        .runtime()
        .opener::<Failing>()
        .expect("discovery declares the failing scope");
    let error = failing
        .builder(Failing { tag: 52 })
        .try_run(|_broken: Arc<Broken>| async { Ok::<_, std::io::Error>(()) })
        .await
        .expect_err("the broken component fails");

    assert!(matches!(error, RunError::Scope(_)));
    assert_eq!(end_of(52).await.kind, ScopeEndKind::Failed);
}

#[tokio::test]
async fn a_run_of_unit_work_completes() {
    let app = app("scope-end-run-unit").await;

    opener(&app)
        .builder(ended(53))
        .run(|| async {})
        .await
        .expect("the run succeeds");

    assert_eq!(end_of(53).await.kind, ScopeEndKind::Completed);
}
