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

/// Collects the fields of every event it sees, as `name=value` strings.
#[derive(Default)]
struct Capture {
    fields: std::sync::Mutex<Vec<String>>,
}

/// Records event fields into the captured output.
struct FieldVisitor<'a>(&'a mut Vec<String>);

impl tracing::field::Visit for FieldVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push(format!("{}={value:?}", field.name()));
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut fields = self
            .fields
            .lock()
            .expect("the capture lock is not poisoned");

        event.record(&mut FieldVisitor(&mut fields));
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

#[test]
fn the_log_event_carries_the_source_text() {
    let capture = std::sync::Arc::new(Capture::default());
    let dispatch = tracing::Dispatch::new(std::sync::Arc::clone(&capture));

    tracing::dispatcher::with_default(&dispatch, || {
        for rejection in rejections() {
            let _ = rejection.into_response();
        }
    });

    let fields = capture
        .fields
        .lock()
        .expect("the capture lock is not poisoned");
    let causes = fields
        .iter()
        .filter(|field| field.starts_with("cause=") && field.contains(SECRET))
        .count();

    assert_eq!(causes, 3, "each source-bearing variant logs its cause text");
}
