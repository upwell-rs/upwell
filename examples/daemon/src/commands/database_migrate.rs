use crate::DaemonApplication;
use crate::components::Database;
use crate::lifecycle::BuildReadiness;
use upwell::{Built, CliCommand, CommandContext, DiError};

const LATEST_SCHEMA_VERSION: u64 = 3;

/// Builds Homeledger and migrates its database without starting the RPC server.
#[derive(clap::Args)]
pub struct DatabaseMigrateCommand {
    /// Print the migration plan without changing the schema version.
    #[arg(long)]
    dry_run: bool,

    /// Schema version to reach.
    #[arg(long, value_name = "VERSION", default_value_t = LATEST_SCHEMA_VERSION)]
    target: u64,
}

impl CliCommand<DaemonApplication> for DatabaseMigrateCommand {
    type Phase = Built;
    type Error = DiError;

    async fn run(
        &self,
        context: CommandContext<DaemonApplication, Self::Phase>,
    ) -> Result<(), Self::Error> {
        let database = context.resolve::<Database>().await?;
        let current = database.verify_schema();
        let verified = context
            .bootstrap()
            .get::<BuildReadiness>()
            .map(|readiness| readiness.schema_version)
            .unwrap_or(current);

        let reconciled = context
            .bootstrap()
            .get::<BuildReadiness>()
            .map_or(0, |readiness| readiness.reconciled_accounts);

        println!("after_build verified schema {verified}");
        println!("after_build reconciled {reconciled} accounts");

        if self.dry_run {
            println!("migration plan: schema {current} -> {}", self.target);
        } else {
            let migrated = database.migrate(self.target);

            println!("database migrated to schema {migrated}");
        }

        Ok(())
    }
}
