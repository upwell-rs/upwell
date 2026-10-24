use cargo_upwell::{
    CommandKind, CommandOutcome, CommandReport, SelectedTargetReport, TOOLING_SCHEMA_VERSION,
};
use upwell_tooling_schema::{Diagnostic, DiagnosticSeverity, DocumentIdentity, SourceLocation};

use super::{detail::write_heading, write_report};
use crate::cli::ReportFormat;

#[test]
fn terminal_report_exposes_live_application_identity() {
    let report = CommandReport {
        schema: TOOLING_SCHEMA_VERSION,
        command: CommandKind::Check,
        outcome: CommandOutcome::Success,
        exit_code: 0,
        target: Some(SelectedTargetReport {
            package: String::from("homeledger"),
            version: String::from("1.0.0"),
            binary: String::from("homeledger"),
            manifest_path: None,
        }),
        application: Some(DocumentIdentity {
            application: String::from("homeledger"),
            ..DocumentIdentity::default()
        }),
        protocol: Some(String::from("homeledger/rpc")),
        framework_version: Some(String::from("0.20.0")),
        checks: Vec::new(),
        diagnostics: Vec::new(),
    };
    let mut output = Vec::new();

    write_report(&report, ReportFormat::Terminal, &mut output).expect("terminal report writes");

    let output = String::from_utf8(output).expect("terminal report is UTF-8");

    assert!(output.contains("Application homeledger (homeledger/rpc)"));
    assert!(output.contains("cargo upwell check passed"));
}

#[test]
fn json_report_is_one_machine_readable_line() {
    let report = CommandReport {
        schema: TOOLING_SCHEMA_VERSION,
        command: CommandKind::Check,
        outcome: CommandOutcome::Success,
        exit_code: 0,
        target: None,
        application: None,
        protocol: None,
        framework_version: None,
        checks: Vec::new(),
        diagnostics: Vec::new(),
    };
    let mut output = Vec::new();

    write_report(&report, ReportFormat::Json, &mut output).expect("JSON report writes");

    let output = String::from_utf8(output).expect("JSON report is UTF-8");

    assert_eq!(output.lines().count(), 1);
    assert!(serde_json::from_str::<serde_json::Value>(output.trim()).is_ok());
}

#[test]
fn terminal_report_preserves_line_without_column() {
    let report = CommandReport {
        schema: TOOLING_SCHEMA_VERSION,
        command: CommandKind::Check,
        outcome: CommandOutcome::BuildFailure,
        exit_code: 4,
        target: None,
        application: None,
        protocol: None,
        framework_version: None,
        checks: Vec::new(),
        diagnostics: vec![Diagnostic {
            code: String::from("rustc/fixture"),
            severity: DiagnosticSeverity::Error,
            message: String::from("fixture failure"),
            sources: vec![SourceLocation {
                file: String::from("src/main.rs"),
                line: Some(12),
                column: None,
                rust_path: None,
            }],
            ..Diagnostic::default()
        }],
    };
    let mut output = Vec::new();

    write_report(&report, ReportFormat::Terminal, &mut output).expect("terminal report writes");

    let output = String::from_utf8(output).expect("terminal report is UTF-8");

    assert!(output.contains("at src/main.rs:12\n"));
}

#[test]
fn terminal_report_escapes_controls_in_shared_diagnostics() {
    let report = CommandReport {
        schema: TOOLING_SCHEMA_VERSION,
        command: CommandKind::Doctor,
        outcome: CommandOutcome::BuildFailure,
        exit_code: 4,
        target: None,
        application: None,
        protocol: None,
        framework_version: None,
        checks: Vec::new(),
        diagnostics: vec![Diagnostic {
            code: String::from("rustc/hostile\u{1b}"),
            severity: DiagnosticSeverity::Error,
            message: String::from("failure\nnext\rreturn\u{1b}[2J"),
            resources: vec![String::from("component\nworker")],
            sources: vec![SourceLocation {
                file: String::from("src/main.rs\r\u{1b}"),
                line: Some(12),
                column: None,
                rust_path: None,
            }],
            fix: Some(String::from("fix\nthis\rnow")),
        }],
    };
    let mut output = Vec::new();

    write_report(&report, ReportFormat::Terminal, &mut output).expect("terminal report writes");

    let output = String::from_utf8(output).expect("terminal report is UTF-8");

    assert!(!output.contains('\u{1b}'));
    assert!(!output.contains('\r'));
    assert!(output.contains(r"error[rustc/hostile\u{1b}]: failure\nnext\rreturn\u{1b}[2J"));
    assert!(output.contains(r"at src/main.rs\r\u{1b}:12"));
}

#[test]
fn terminal_report_escapes_bidirectional_formatting_controls() {
    let report = CommandReport {
        schema: TOOLING_SCHEMA_VERSION,
        command: CommandKind::Doctor,
        outcome: CommandOutcome::ValidationFailure,
        exit_code: 1,
        target: None,
        application: None,
        protocol: None,
        framework_version: None,
        checks: Vec::new(),
        diagnostics: vec![Diagnostic {
            code: String::from("fixture/\u{202e}code"),
            severity: DiagnosticSeverity::Error,
            message: String::from("before\u{2066}after"),
            ..Diagnostic::default()
        }],
    };
    let mut output = Vec::new();

    write_report(&report, ReportFormat::Terminal, &mut output).expect("terminal report writes");

    let output = String::from_utf8(output).expect("terminal report is UTF-8");

    assert!(!output.contains('\u{202e}'));
    assert!(!output.contains('\u{2066}'));
    assert!(output.contains(r"fixture/\u{202e}code"));
    assert!(output.contains(r"before\u{2066}after"));
}

#[test]
fn hostile_heading_is_one_line_and_preserves_unicode() {
    let mut output = Vec::new();

    write_heading(&mut output, "Café\u{1b}[31m\nnext\rline", false).expect("heading writes");

    let output = String::from_utf8(output).expect("heading is UTF-8");

    assert_eq!(output.lines().count(), 1);
    assert_eq!(output, "Café\\u{1b}[31m\\nnext\\rline\n");
}
