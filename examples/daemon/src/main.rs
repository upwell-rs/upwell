//! Homeledger, a complete generated Upwell application host for a household ledger.
//!
//! The named `app!` declaration generates the Clap parser, lifecycle dispatcher, and
//! `DaemonApplication::run()` process entry point. It also demonstrates application-specific
//! customization of framework-owned CLI slots without replacing generated bootstrap or startup.
//!
//! ```text
//! cargo run -p upwell-example-daemon -- --help
//! cargo run -p upwell-example-daemon -- inspect registry --verbose
//! cargo run -p upwell-example-daemon -- database migrate --dry-run
//! cargo run -p upwell-example-daemon -- operator-context --operator release-bot
//! cargo run -p upwell-example-daemon -- --environment production --log-filter homeledger=trace --log-format json run
//! cargo run -p upwell-example-daemon --
//! ```
//!
//! The declaration changes `--config` to `--config-dir`, `--profile` to `--environment`, and the
//! generated `serve` command to `run`, while retaining visible compatibility aliases. The config
//! directory and `development` profile declared with Clap's exact default settings are shown in
//! help and make commands runnable from the workspace. Generated bootstrap retains their
//! `DefaultValue` source. Config location resolves explicit CLI, `UPWELL_CONFIG`, parser default,
//! then the platform directory; profiles resolve explicit CLI, `UPWELL_PROFILES`, parser
//! defaults, then an empty list. Loaded profile/base config participates later when logging and
//! color settings are resolved.
//! `--color` is deliberately disabled because Homeledger's logging destination owns ANSI policy
//! through `logging.ansi`/`NO_COLOR`.
//!
//! With no subcommand, the generated CLI intentionally defaults to `run` and serves until Ctrl-C.
//! `operator-context` comes from a statically installed plugin and stops after setup; `inspect
//! registry` reaches `PreBuild` without constructing components; `database migrate` reaches `Built`
//! so it can resolve the database but never starts the server. The setup and `after_build` hooks
//! retain bootstrap provenance and verify the database schema for both commands and serving.
//! Homeledger's example-local RPC definition also exposes audit-policy defaults: the declaration
//! replaces the household policy with its compliance policy and suppresses optional audit export.
//! The selected protocol still delegates preparation, runtime construction, and serving to RPC.
//! The `after_build` hook also reconciles each household account in its own instance of the
//! user-authored `AccountReconcile` scope.

mod audit;
mod commands;
mod components;
mod lifecycle;
mod operations;
mod protocol;
mod reconcile;
mod service;

use crate::commands::{DatabaseMigrateCommand, InspectRegistryCommand, OutputArgs};
use crate::components::DatabaseConfig;
use crate::lifecycle::{after_build, setup};
use crate::operations::HomeledgerOperationsPlugin;
use crate::protocol::{
    AUDIT_EXPORT_SLOT, AUDIT_POLICY_SLOT, ComplianceAuditPolicyPlugin, HomeledgerRpc,
};
use upwell::{Cfg, LogFormat, LoggingConfig, ServerConfig, TcpTransport, app};

app! {
    /// A generated Homeledger daemon host with lifecycle-aware administration commands.
    app DaemonApplication {
        name: "homeledger",
        protocol: HomeledgerRpc,
        configs: [
            DatabaseConfig => "homeledger.database.reader",
            DatabaseConfig => "homeledger.database.writer",
            ServerConfig => "homeledger.server",
            LoggingConfig => "logging",
        ],
        cli: {
            config: {
                name: "config-dir",
                visible_aliases: ["config"],
                help: "Homeledger configuration file or directory.",
                value_name: "CONFIG_DIR",
                default_value: "examples/daemon/config",
            },
            profile: {
                name: "environment",
                short: 'e',
                visible_aliases: ["profile"],
                help: "Ordered Homeledger environment overlay; may be repeated.",
                value_name: "ENVIRONMENT",
                default_values_t: [String::from("development")],
            },
            log: {
                name: "log-filter",
                visible_aliases: ["log"],
                help: "Override the configured tracing filter.",
                value_name: "DIRECTIVES",
            },
            log_format: {
                help: "Override the configured tracing formatter.",
                default_value_t: LogFormat::Compact,
            },
            color: false,
            serve: {
                name: "run",
                visible_aliases: ["serve"],
                help: "Run the Homeledger RPC service until shutdown.",
                default_command: true,
            },
        },
        plugins: [
            HomeledgerOperationsPlugin,
            replace AUDIT_POLICY_SLOT => ComplianceAuditPolicyPlugin,
            suppress AUDIT_EXPORT_SLOT,
        ],
        args: {
            output: OutputArgs,
        },
        commands: {
            /// Inspect Homeledger's validated application metadata.
            #[command(alias = "show", visible_alias = "describe", display_order = 10)]
            inspect: {
                /// Print the prepared registry and plugin plan.
                registry: InspectRegistryCommand,
            },
            /// Run Homeledger database administration commands.
            #[command(alias = "db", display_order = 20)]
            database: {
                /// Build Homeledger and migrate its database schema.
                migrate: DatabaseMigrateCommand,
            },
        },
        setup = setup,
        after_build = after_build,
        serve(_context, app, server: Cfg<ServerConfig>) {
            let server = server.snapshot();
            let transport = TcpTransport::bind((server.bind.as_str(), server.port)).await?;

            println!(
                "Homeledger listening on {}:{}; press Ctrl-C to stop",
                server.bind, server.port
            );

            app.serve(transport).await?;

            Ok::<(), upwell::daemon::Error>(())
        },
    }
}

#[tokio::main]
async fn main() -> Result<(), upwell::CliError> {
    DaemonApplication::run().await
}

#[cfg(test)]
#[path = "main/tests.rs"]
mod tests;
