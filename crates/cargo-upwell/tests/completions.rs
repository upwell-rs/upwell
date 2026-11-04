mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use upwell_test_utils::{TempFixture, run_command};

#[test]
fn every_supported_shell_gets_dynamic_registration() {
    for shell in ["bash", "elvish", "fish", "powershell", "zsh"] {
        let output = cargo_upwell(
            common::workspace_root().as_path(),
            ["completions", "generate", shell],
            None,
        );

        assert_success(&output, shell);
        let registration = String::from_utf8(output.stdout).expect("registration is UTF-8");

        assert!(registration.contains("cargo-upwell"));
        assert!(registration.contains("COMPLETE"));
        if shell == "powershell" {
            assert!(!registration.contains("Invoke-Expression"));
        }
    }
}

#[test]
fn fish_registration_completes_direct_and_cargo_subcommand_forms() {
    if Command::new("fish").arg("--version").output().is_err() {
        return;
    }
    let registration = cargo_upwell(
        common::workspace_root().as_path(),
        ["completions", "generate", "fish"],
        None,
    );
    assert_success(&registration, "Fish registration");
    let registration = String::from_utf8(registration.stdout).expect("registration is UTF-8");

    for commandline in ["cargo-upwell ", "cargo upwell "] {
        let mut command = Command::new("fish");

        command
            .arg("-c")
            .arg(format!("{registration}\ncomplete -C '{commandline}'"));
        let output = run_command("Fish completion", &mut command);

        assert_success(&output, commandline);
        assert!(String::from_utf8_lossy(&output.stdout).contains("inspect\t"));
    }
}

#[test]
fn refresh_populates_workspace_resource_candidates() {
    if std::env::var_os("UPWELL_SKIP_GENERATED_PROJECT_BUILDS").is_some() {
        return;
    }
    let _guard = common::cargo_build_lock();
    let fixture = TempFixture::new_in(
        Path::new(env!("CARGO_TARGET_TMPDIR")),
        "cargo-upwell-completion-cache",
    );
    let project = fixture.child("generated");
    let cache = fixture.child("cache");
    let template = fixture.child("template");

    std::fs::create_dir_all(&template).expect("template directory exists");
    std::fs::write(
        template.join("Cargo.toml.liquid"),
        r#"[package]
name = "{{ project-name }}"
version = "0.1.0"
edition = "2024"

[workspace]

[features]
default = ["cli"]
cli = ["upwell/cli", "dep:clap"]
metrics = []

[dependencies]
upwell = {{ upwell_dependency }}
clap = { version = "4", optional = true, features = ["derive"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
"#,
    )
    .expect("template manifest is written");
    std::fs::create_dir_all(template.join("src")).expect("template source directory exists");
    std::fs::write(
        template.join("src/main.rs.liquid"),
        r#"#[upwell::component(by_value)]
#[derive(Clone)]
struct Worker;

#[derive(Default)]
struct WorkerPlugin;

impl upwell::Plugin for WorkerPlugin {
    const ID: upwell::PluginId = upwell::namespaced_id!(upwell::PluginId, "fixture/worker");

    fn contribute(self, contributions: &mut upwell::PluginContributions) {
        contributions.component::<Worker>(upwell::namespaced_id!(
            upwell::ContributionId,
            "fixture/worker-component"
        ));
    }
}

upwell::app! {
    app Application {
        name: "completion-fixture",
        protocol: (),
        plugins: [WorkerPlugin],
        cli: { serve: false },
    }
}

#[tokio::main]
async fn main() -> Result<(), upwell::CliError> {
    Application::run().await
}
"#,
    )
    .expect("template source is written");
    std::fs::write(
        template.join("cargo-generate.toml"),
        r#"[placeholders.upwell_dependency]
type = "string"
prompt = "Upwell dependency"
"#,
    )
    .expect("template configuration is written");

    let generated = cargo_upwell(
        fixture.path(),
        [
            "init",
            project.to_str().expect("project path is UTF-8"),
            "--template-path",
            template.to_str().expect("template path is UTF-8"),
            "--upwell-path",
            common::workspace_root()
                .to_str()
                .expect("workspace path is UTF-8"),
            "--no-vcs",
        ],
        Some(&cache),
    );
    assert_success(&generated, "fixture generation");

    let refreshed = cargo_upwell(
        &project,
        ["completions", "refresh", "--features", "metrics"],
        Some(&cache),
    );
    assert_success(&refreshed, "completion refresh");
    let snapshot = completion_snapshot(&cache);

    assert!(snapshot.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        assert_eq!(
            std::fs::metadata(&snapshot)
                .expect("snapshot metadata exists")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    let resources = complete(&project, &cache, ["cargo-upwell", "explain", "component:w"]);
    let features = complete(
        &project,
        &cache,
        ["cargo-upwell", "check", "--features", "m"],
    );

    assert!(
        resources.contains("component\\:worker"),
        "unexpected resource completions: {resources:?}"
    );
    assert!(
        features.starts_with("metrics"),
        "unexpected feature completions: {features:?}"
    );
}

fn completion_snapshot(cache: &Path) -> PathBuf {
    let workspace_cache = std::fs::read_dir(cache)
        .expect("cache root is readable")
        .next()
        .expect("workspace cache exists")
        .expect("workspace cache entry is readable")
        .path();

    workspace_cache.join("active.json")
}

fn complete<const N: usize>(current_dir: &Path, cache: &Path, words: [&str; N]) -> String {
    let mut command = Command::new(cargo_upwell_executable());

    command
        .current_dir(current_dir)
        .env("UPWELL_COMPLETION_CACHE_DIR", cache)
        .env(
            "CARGO",
            current_dir.join("cargo-must-not-run-during-completion"),
        )
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", (N - 1).to_string())
        .arg("--")
        .args(words);
    let output = run_command("cargo-upwell completion", &mut command);

    assert_success(&output, "dynamic completion");
    String::from_utf8(output.stdout).expect("completion output is UTF-8")
}

fn cargo_upwell<I, S>(current_dir: &Path, arguments: I, cache: Option<&Path>) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut command = Command::new(cargo_upwell_executable());

    command.current_dir(current_dir).args(arguments);
    if let Some(cache) = cache {
        command.env("UPWELL_COMPLETION_CACHE_DIR", cache);
    }

    run_command("cargo-upwell", &mut command)
}

fn assert_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn cargo_upwell_executable() -> PathBuf {
    std::env::var_os("NEXTEST_BIN_EXE_cargo-upwell")
        .or_else(|| std::env::var_os("CARGO_BIN_EXE_cargo-upwell"))
        .map(PathBuf::from)
        .expect("cargo-upwell test executable is available")
}
