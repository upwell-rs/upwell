use super::*;
use clap::{CommandFactory as _, Parser as _};
use upwell::{ColorChoice, LogFormat, Plugin, resolve_host_plugin_catalog};
use upwell_test_utils::{TempFixture, path_ends_with_components};

static WORKSPACE_CWD: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn generated_help_matches_normalized_snapshot() {
    let mut plugins = resolve_host_plugin_catalog::<DaemonApplication>()
        .expect("Homeledger plugin catalog resolves");
    let mut command = DaemonApplication::__upwell_compose_cli(&mut plugins)
        .expect("Homeledger CLI composes")
        .term_width(100);
    let actual = normalize_help(command.render_long_help().to_string());
    let expected = normalize_help(include_str!("../../snapshots/help.txt").to_owned());

    assert_eq!(actual, expected);
}

#[test]
fn generated_parser_preserves_customized_slots_and_no_command_default() {
    let parser = DaemonApplicationCli::command();
    let parsed = DaemonApplicationCli::try_parse_from(["homeledger"])
        .expect("no command retains generated run default");

    assert!(parsed.command.is_none());
    assert!(
        parser
            .get_subcommands()
            .any(|command| command.get_name() == "run")
    );
}

#[test]
fn named_host_resolves_static_install_replacement_and_suppression() {
    let plugins = resolve_host_plugin_catalog::<DaemonApplication>()
        .expect("Homeledger plugin catalog resolves");
    let plan = plugins.plan();
    let replacement = plan
        .replacements()
        .iter()
        .find(|decision| decision.slot() == crate::protocol::AUDIT_POLICY_SLOT)
        .expect("audit policy replacement is retained");
    let suppression = plan
        .suppressions()
        .iter()
        .find(|decision| decision.slot() == crate::protocol::AUDIT_EXPORT_SLOT)
        .expect("audit export suppression is retained");

    assert!(
        plan.plugin(crate::operations::HomeledgerOperationsPlugin::ID)
            .is_some()
    );
    assert_eq!(
        replacement.replaced(),
        crate::protocol::HouseholdAuditPolicyPlugin::ID
    );
    assert_eq!(
        replacement.replacement(),
        crate::protocol::ComplianceAuditPolicyPlugin::ID
    );
    assert_eq!(
        suppression.suppressed(),
        crate::protocol::AuditExportPlugin::ID
    );
    assert!(plan.slot(crate::protocol::AUDIT_EXPORT_SLOT).is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn tooling_probe_projects_real_homeledger_plan_without_building_runtime() {
    let _cwd = workspace_cwd();
    let _environment = clean_bootstrap_environment();

    crate::components::reset_database_builds();
    crate::protocol::reset_protocol_builds();

    let target = upwell::tooling::ProbeTargetIdentity::new(
        upwell::tooling::PackageIdentity {
            name: String::from(env!("CARGO_PKG_NAME")),
            version: Some(String::from(env!("CARGO_PKG_VERSION"))),
            manifest_path: Some(format!("{}/Cargo.toml", env!("CARGO_MANIFEST_DIR"))),
        },
        upwell::tooling::BinaryTargetIdentity {
            name: String::from("upwell-example-daemon"),
        },
    )
    .expect("explicit Homeledger tooling target is valid");
    let envelope = DaemonApplication::tooling_probe(target)
        .await
        .expect("generated Homeledger declaration identity validates");
    let upwell::tooling::ProbeOutcome::Success { document } = envelope.outcome else {
        panic!(
            "Homeledger tooling probe unexpectedly failed: {:#?}",
            envelope.outcome
        );
    };

    assert_eq!(document.identity.application, "homeledger");
    assert_eq!(
        document
            .identity
            .package
            .as_ref()
            .map(|package| package.name.as_str()),
        Some("upwell-example-daemon")
    );
    assert_eq!(
        document
            .identity
            .binary
            .as_ref()
            .map(|binary| binary.name.as_str()),
        Some("upwell-example-daemon")
    );
    assert!(document.identity.source.as_ref().is_some_and(|source| {
        path_ends_with_components(&source.file, &["examples", "daemon", "src", "main.rs"])
    }));
    assert_eq!(crate::components::database_builds(), 0);
    assert_eq!(crate::protocol::protocol_builds(), 0);
    let rpc = document
        .resources
        .iter()
        .find(|resource| resource.id == "protocol:homeledger/rpc")
        .expect("Homeledger RPC protocol resource exists");
    let route = document
        .resources
        .iter()
        .find(|resource| resource.labels.get("kind").map(String::as_str) == Some("rpc-route"))
        .expect("Homeledger RPC route resource exists");

    assert_eq!(
        rpc.display
            .as_ref()
            .and_then(|display| display.label.as_deref()),
        Some("RPC")
    );
    assert_eq!(
        route
            .display
            .as_ref()
            .and_then(|display| display.label.as_deref()),
        Some("unary LedgerService.record_transaction")
    );

    let cli = document
        .cli
        .expect("Homeledger probe includes effective CLI metadata");
    let config = cli
        .root
        .arguments
        .iter()
        .find(|argument| argument.id == "config")
        .expect("canonical config argument exists");
    let profile = cli
        .root
        .arguments
        .iter()
        .find(|argument| argument.id == "profiles")
        .expect("canonical profile argument exists");
    let log_format = cli
        .root
        .arguments
        .iter()
        .find(|argument| argument.id == "log_format")
        .expect("canonical log format argument exists");
    let review_ticket = cli
        .root
        .arguments
        .iter()
        .find(|argument| argument.id == "audit_review_ticket")
        .expect("compliance plugin argument exists");
    let serve = cli
        .root
        .commands
        .iter()
        .find(|command| command.id.as_deref() == Some("serve"))
        .expect("canonical serve command exists");
    let provider = cli
        .providers
        .iter()
        .find(|provider| provider.contribution == "homeledger/compliance-audit-args")
        .expect("compliance plugin CLI provider provenance exists");

    assert_eq!(config.long.as_deref(), Some("config-dir"));
    assert_eq!(config.default_values, ["examples/daemon/config"]);
    assert_eq!(profile.long.as_deref(), Some("environment"));
    assert_eq!(profile.default_values, ["development"]);
    assert_eq!(log_format.default_values, ["compact"]);
    assert_eq!(serve.name, "run");
    assert_eq!(serve.visible_aliases, ["serve"]);
    assert_eq!(cli.default_command.as_deref(), Some("serve"));
    assert_eq!(
        provider.contributor,
        "plugin:homeledger/audit-policy-compliance"
    );
    assert!(matches!(
        review_ticket.owner,
        upwell::tooling::CliOwner::Plugin { provider: ref owner } if owner == &provider.id
    ));

    assert!(document.relationships.iter().any(|relationship| {
        relationship.kind == upwell::tooling::RelationshipKind::Replaces
            && relationship.from == "plugin:homeledger/audit-policy-compliance"
            && relationship.to == "plugin:homeledger/audit-policy-household"
            && relationship.labels["slot"] == "homeledger/audit-policy"
    }));
    assert!(document.relationships.iter().any(|relationship| {
        relationship.kind == upwell::tooling::RelationshipKind::Suppresses
            && relationship.from == "suppression:homeledger/audit-export"
            && relationship.to == "plugin:homeledger/audit-export"
    }));
    assert!(document.resources.iter().any(|resource| {
        resource.id == "plugin:homeledger/audit-policy-compliance"
            && resource.provenance.as_ref().is_some_and(|provenance| {
                provenance.origin.as_deref() == Some("application-declaration")
            })
    }));
    assert!(document.resources.iter().any(|resource| {
        resource.id == "plugin:homeledger/audit-export"
            && resource.labels.get("decision").map(String::as_str) == Some("suppressed")
    }));
    assert!(document.resources.iter().any(|resource| {
        resource.id.contains("homeledger/audit-policy-config")
            && resource.labels.get("decision").map(String::as_str) == Some("applied")
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn generated_host_resolves_parser_defaults_and_loaded_config() {
    let _cwd = workspace_cwd();
    let _environment = clean_bootstrap_environment();

    DaemonApplication::run_with(["homeledger", "operator-context"])
        .await
        .expect("Homeledger setup command resolves generated bootstrap");

    let state = crate::lifecycle::take_test_bootstrap();

    assert_eq!(state.config_path, "examples/daemon/config");
    assert_eq!(state.profiles, ["development"]);
    assert_eq!(state.log, "debug,homeledger=trace,upwell=debug");
    assert_eq!(state.log_format, LogFormat::Compact);
    assert_eq!(state.color, ColorChoice::Never);
}

#[tokio::test(flavor = "current_thread")]
async fn after_build_reconciles_each_account_in_its_own_scope_instance() {
    let _cwd = workspace_cwd();
    let _environment = clean_bootstrap_environment();

    DaemonApplication::run_with(["homeledger", "database", "migrate", "--dry-run"])
        .await
        .expect("after_build reconciles every account before the command runs");

    assert_eq!(crate::lifecycle::take_reconciled_accounts(), 3);
}

#[tokio::test(flavor = "current_thread")]
async fn generated_host_cli_values_override_environment_config_and_defaults() {
    let _cwd = workspace_cwd();
    let config = TestConfig::new("cli", "cli", Some("debug"), Some("compact"), Some(false));
    let _environment = TestEnvironment::set(&[
        ("UPWELL_CONFIG", Some("environment-should-not-win")),
        ("UPWELL_PROFILES", Some("environment")),
        ("RUST_LOG", Some("error,homeledger=warn")),
        ("UPWELL_LOG_FORMAT", Some("pretty")),
        ("NO_COLOR", None),
        ("CLICOLOR_FORCE", None),
    ]);

    DaemonApplication::run_with(vec![
        String::from("homeledger"),
        String::from("operator-context"),
        String::from("--config"),
        config.path().display().to_string(),
        String::from("--profile"),
        String::from("cli"),
        String::from("--log"),
        String::from("trace,homeledger=debug"),
        String::from("--log-format"),
        String::from("json"),
    ])
    .await
    .expect("Homeledger explicit bootstrap options resolve");

    let state = crate::lifecycle::take_test_bootstrap();

    assert_eq!(state.config_path, config.path().display().to_string());
    assert_eq!(state.profiles, ["cli"]);
    assert_eq!(state.log, "trace,homeledger=debug");
    assert_eq!(state.log_format, LogFormat::Json);
    assert_eq!(state.color, ColorChoice::Never);
}

#[tokio::test(flavor = "current_thread")]
async fn generated_host_environment_overrides_config_and_parser_defaults() {
    let _cwd = workspace_cwd();
    let config = TestConfig::new(
        "environment",
        "environment",
        Some("debug"),
        Some("compact"),
        Some(false),
    );
    let config_path = config.path().display().to_string();
    let _environment = TestEnvironment::set(&[
        ("UPWELL_CONFIG", Some(&config_path)),
        ("UPWELL_PROFILES", Some("environment")),
        ("RUST_LOG", Some("error,homeledger=warn")),
        ("UPWELL_LOG_FORMAT", Some("pretty")),
        ("NO_COLOR", None),
        ("CLICOLOR_FORCE", Some("1")),
    ]);

    DaemonApplication::run_with(["homeledger", "operator-context"])
        .await
        .expect("Homeledger environment bootstrap options resolve");

    let state = crate::lifecycle::take_test_bootstrap();

    assert_eq!(state.config_path, config_path);
    assert_eq!(state.profiles, ["environment"]);
    assert_eq!(state.log, "error,homeledger=warn");
    assert_eq!(state.log_format, LogFormat::Pretty);
    assert_eq!(state.color, ColorChoice::Always);
}

#[tokio::test(flavor = "current_thread")]
async fn generated_host_parser_defaults_fill_absent_config_sources() {
    let _cwd = workspace_cwd();
    let config = TestConfig::new("fallback", "development", None, None, None);
    let config_path = config.path().display().to_string();
    let _environment = TestEnvironment::set(&[
        ("UPWELL_CONFIG", Some(&config_path)),
        ("UPWELL_PROFILES", None),
        ("RUST_LOG", None),
        ("UPWELL_LOG_FORMAT", None),
        ("NO_COLOR", None),
        ("CLICOLOR_FORCE", None),
    ]);

    DaemonApplication::run_with(["homeledger", "operator-context"])
        .await
        .expect("Homeledger parser defaults resolve");

    let state = crate::lifecycle::take_test_bootstrap();

    assert_eq!(state.config_path, config_path);
    assert_eq!(state.profiles, ["development"]);
    assert_eq!(state.log, "info");
    assert_eq!(state.log_format, LogFormat::Compact);
    assert_eq!(state.color, ColorChoice::Always);
}

/// Clears every environment variable the generated bootstrap reads, so a test sees only its
/// parser defaults and loaded config.
fn clean_bootstrap_environment() -> TestEnvironment {
    TestEnvironment::set(&[
        ("UPWELL_CONFIG", None),
        ("UPWELL_PROFILES", None),
        ("RUST_LOG", None),
        ("UPWELL_LOG_FORMAT", None),
        ("NO_COLOR", None),
        ("CLICOLOR_FORCE", None),
    ])
}

fn workspace_cwd() -> WorkspaceCwd {
    let lock = WORKSPACE_CWD
        .lock()
        .expect("workspace CWD lock is available");
    let original = std::env::current_dir().expect("current directory is available");
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("daemon example is nested under the workspace");

    std::env::set_current_dir(workspace).expect("switch to workspace root");

    WorkspaceCwd {
        _lock: lock,
        original,
    }
}

/// Restores the process directory after a workspace-relative generated-host test.
struct WorkspaceCwd {
    _lock: std::sync::MutexGuard<'static, ()>,
    original: std::path::PathBuf,
}

impl Drop for WorkspaceCwd {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.original).expect("restore original current directory");
    }
}

/// Restores bootstrap environment variables after a single-threaded generated-host test.
struct TestEnvironment {
    original: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl TestEnvironment {
    fn set(values: &[(&'static str, Option<&str>)]) -> Self {
        let original = values
            .iter()
            .map(|(name, _)| (*name, std::env::var_os(name)))
            .collect();

        for (name, value) in values {
            // SAFETY: these current-thread tests hold WORKSPACE_CWD for the guard's full lifetime,
            // serializing every environment mutation and generated bootstrap read in this module.
            #[allow(unsafe_code)]
            unsafe {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }

        Self { original }
    }
}

impl Drop for TestEnvironment {
    fn drop(&mut self) {
        for (name, value) in &self.original {
            // SAFETY: the guard still holds WORKSPACE_CWD while restoring the process environment.
            #[allow(unsafe_code)]
            unsafe {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }
}

/// Temporary base/profile configuration used to distinguish bootstrap precedence sources.
struct TestConfig {
    fixture: TempFixture,
}

impl TestConfig {
    fn new(
        test: &str,
        profile: &str,
        level: Option<&str>,
        format: Option<&str>,
        ansi: Option<bool>,
    ) -> Self {
        let fixture = TempFixture::new(&format!("upwell-homeledger-{test}-"));
        let mut profile_config = String::from("[logging]\n");

        if let Some(level) = level {
            profile_config.push_str(&format!("level = \"{level}\"\n"));
        }

        if let Some(format) = format {
            profile_config.push_str(&format!("format = \"{format}\"\n"));
        }

        if let Some(ansi) = ansi {
            profile_config.push_str(&format!("ansi = {ansi}\n"));
        }

        fixture.write(
            "application.toml",
            "[logging]\nlevel = \"info\"\nansi = true\n",
        );
        fixture.write(format!("application-{profile}.toml"), profile_config);

        Self { fixture }
    }

    fn path(&self) -> &std::path::Path {
        self.fixture.path()
    }
}

fn normalize_help(help: String) -> String {
    let normalized = help.replace("\r\n", "\n");
    let lines = normalized.lines().map(str::trim_end).collect::<Vec<_>>();

    format!("{}\n", lines.join("\n").trim())
}
