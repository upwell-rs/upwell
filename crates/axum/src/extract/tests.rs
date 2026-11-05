use std::error::Error as _;

use axum::body;

use super::*;

const SECRET: &str = "hunter2-secret-token";

fn di_error() -> upwell_di::Error {
    upwell_di::Error::DuplicateComponentId(SECRET.to_owned())
}

fn app_error() -> upwell_app::Error {
    upwell_app::Error::Di(di_error())
}

fn rejections() -> Vec<InjectRejection> {
    vec![
        InjectRejection::MissingScope,
        InjectRejection::ScopeFailed(app_error()),
        InjectRejection::SeedFailed(di_error()),
        InjectRejection::Unresolved {
            type_name: "Missing",
            source: di_error(),
        },
    ]
}

#[test]
fn a_missing_scope_has_no_source() {
    let rejection = InjectRejection::MissingScope;

    assert!(rejection.source().is_none());
}

#[test]
fn scope_failed_carries_the_typed_app_error() {
    let rejection = InjectRejection::ScopeFailed(app_error());

    let source = rejection.source().expect("the open failure is the source");

    assert!(source.downcast_ref::<upwell_app::Error>().is_some());
}

#[test]
fn seed_failed_carries_the_typed_di_error() {
    let rejection = InjectRejection::SeedFailed(di_error());

    let source = rejection.source().expect("the seed failure is the source");

    assert!(source.downcast_ref::<upwell_di::Error>().is_some());
}

#[test]
fn unresolved_names_the_type_and_carries_the_typed_di_error() {
    let rejection = InjectRejection::Unresolved {
        type_name: "Missing",
        source: di_error(),
    };

    let source = rejection.source().expect("the DI failure is the source");

    assert!(source.downcast_ref::<upwell_di::Error>().is_some());
    assert_eq!(rejection.to_string(), "failed to inject `Missing`");
}

#[tokio::test]
async fn every_rejection_answers_an_empty_500_without_the_source_text() {
    for rejection in rejections() {
        let response = rejection.into_response();
        let status = response.status();
        let bytes = body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body reads");

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(bytes.is_empty(), "the body stays empty");
    }
}
