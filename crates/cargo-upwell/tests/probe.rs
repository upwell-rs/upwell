mod common;

use cargo_upwell::{
    CancellationToken, DiscoveryRequest, FeatureSelection, ProbeOptions, ResourceExplanation,
    run_probe, run_probe_with_options,
};
use common::{cargo_build_lock, workspace_root};
use upwell_tooling_schema::{
    ConfigAssignment, PreviewAction, PreviewEvaluation, PreviewOutcome, PreviewStrategy,
    ProbeOutcome, TransitionPreviewRequest,
};

#[test]
fn builds_and_consumes_a_structured_failure_probe() {
    let workspace = workspace_root();
    let request = DiscoveryRequest {
        manifest_path: Some(workspace.join("Cargo.toml")),
        current_dir: Some(workspace.clone()),
        package: Some(String::from("upwell")),
        binary: Some(String::from("tooling_probe_fixture")),
        features: FeatureSelection {
            features: vec![String::from("cli"), String::from("tooling")],
            ..FeatureSelection::default()
        },
        ..DiscoveryRequest::default()
    };
    let _lock = cargo_build_lock();

    let result = run_probe(&request, &CancellationToken::default())
        .expect("failure envelope is a completed probe result");
    let ProbeOutcome::Failure { failure } = &result.probe.envelope.outcome else {
        panic!("panicking fixture unexpectedly succeeded");
    };

    assert_eq!(result.target.package_name, "upwell");
    assert_eq!(result.target.binary_name, "tooling_probe_fixture");
    assert_eq!(result.probe.evidence.status.code, Some(1));
    assert_eq!(failure.diagnostics[0].code, "upwell/tooling-panic");
    assert!(
        result
            .build
            .target_directory
            .starts_with(workspace.join("target/upwell"))
    );

    for bytes in [
        result.probe.evidence.stdout.as_slice(),
        result.probe.evidence.stderr.as_slice(),
    ] {
        assert!(!String::from_utf8_lossy(bytes).contains("probe-process-secret"));
    }
}

#[test]
fn discovers_and_probes_the_homeledger_application() {
    let workspace = workspace_root();
    let request = DiscoveryRequest {
        manifest_path: Some(workspace.join("examples/daemon/Cargo.toml")),
        current_dir: Some(workspace.clone()),
        package: Some(String::from("upwell-example-daemon")),
        binary: Some(String::from("upwell-example-daemon")),
        ..DiscoveryRequest::default()
    };
    let _lock = cargo_build_lock();

    let result = run_probe(&request, &CancellationToken::default())
        .expect("Homeledger target builds and probes");
    let ProbeOutcome::Success { document } = &result.probe.envelope.outcome else {
        panic!("Homeledger probe unexpectedly failed");
    };

    assert_eq!(document.identity.application, "homeledger");
    assert_eq!(result.target.package_name, "upwell-example-daemon");
    assert_eq!(result.target.binary_name, "upwell-example-daemon");
    assert!(result.probe.evidence.status.success);
    assert!(result.probe.evidence.stdout.is_empty());
}

#[test]
fn explains_the_authenticator_selection_and_the_impact_of_flipping_its_flag() {
    let workspace = workspace_root();
    let request = DiscoveryRequest {
        manifest_path: Some(workspace.join("Cargo.toml")),
        current_dir: Some(workspace.clone()),
        package: Some(String::from("upwell")),
        binary: Some(String::from("tooling_preview_fixture")),
        features: FeatureSelection {
            features: vec![String::from("cli"), String::from("tooling")],
            ..FeatureSelection::default()
        },
        ..DiscoveryRequest::default()
    };
    let options = ProbeOptions {
        transition_preview: Some(TransitionPreviewRequest {
            assignments: vec![ConfigAssignment {
                path: String::from("auth.mode"),
                value: String::from("default"),
            }],
        }),
        ..ProbeOptions::default()
    };
    let _lock = cargo_build_lock();

    let result = run_probe_with_options(&request, &CancellationToken::default(), options)
        .expect("the preview fixture builds and probes");
    let ProbeOutcome::Success { document } = &result.probe.envelope.outcome else {
        panic!("the preview fixture probe failed");
    };
    let explanation =
        ResourceExplanation::query(document, "type:dyn tooling_preview_fixture::Authenticator")
            .expect("the Authenticator trait explains");
    let selection = explanation
        .selection
        .iter()
        .map(|selection| {
            (
                selection.component.as_deref(),
                selection.effective,
                selection.role.as_deref(),
            )
        })
        .collect::<Vec<_>>();
    let preview = explanation
        .transition
        .as_ref()
        .expect("the requested preview is explained");
    let action = |id: &str| {
        preview
            .components
            .iter()
            .find(|component| component.component == id)
            .map(|component| (component.action, component.strategy))
    };

    assert_eq!(
        selection,
        [
            (
                Some("component:customauthenticator"),
                true,
                Some("conditional")
            ),
            (
                Some("component:defaultauthenticator"),
                true,
                Some("fallback")
            ),
        ]
    );
    assert_eq!(
        explanation.selection[0]
            .resolved_by
            .iter()
            .map(|resolved| resolved.consumer.as_str())
            .collect::<Vec<_>>(),
        ["component:authbanner", "component:logingate"],
        "both consumers resolve to the primary custom provider"
    );
    assert!(explanation.selection[1].resolved_by.is_empty());
    assert_eq!(
        explanation.selection[0].conditions[0].outcome.as_deref(),
        Some("holds"),
        "the pure `#[when]` condition is decided by tooling"
    );
    assert_eq!(preview.outcome, PreviewOutcome::Planned);
    assert_eq!(preview.evaluation, PreviewEvaluation::Complete);
    assert_eq!(
        action("customauthenticator"),
        Some((PreviewAction::Remove, None))
    );
    assert_eq!(
        action("authbanner"),
        Some((PreviewAction::Replace, Some(PreviewStrategy::Reconstruct)))
    );
    assert_eq!(
        action("logingate"),
        Some((PreviewAction::RebindLive, Some(PreviewStrategy::Retain)))
    );
    assert_eq!(preview.displaced, ["authbanner", "customauthenticator"]);
}
