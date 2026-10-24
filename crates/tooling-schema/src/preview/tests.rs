use super::*;

#[test]
fn an_empty_request_object_decodes_as_the_default_request() {
    let request = TransitionPreviewRequest::from_env_value("{}").expect("an empty object decodes");

    assert_eq!(request, TransitionPreviewRequest::default());
}

#[test]
fn a_request_round_trips_through_its_environment_value() {
    let request = TransitionPreviewRequest {
        assignments: vec![ConfigAssignment {
            path: String::from("auth.mode"),
            value: String::from("\"custom\""),
        }],
    };

    let decoded = TransitionPreviewRequest::from_env_value(&request.to_env_value())
        .expect("the encoded request decodes");

    assert_eq!(decoded, request);
}
