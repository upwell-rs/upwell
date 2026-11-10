//! Homeledger work performed around generated host construction.

use crate::components::Database;
use crate::operations::OperationsArgs;
use crate::protocol::{ComplianceAuditArgs, HomeledgerRpc};
use crate::reconcile::{AccountReconcile, reconcile_accounts};
use upwell::{App, BootstrapContext, DiError, resolve_host_dependency};

#[cfg(test)]
use upwell::{ColorChoice, LogFormat};

#[cfg(test)]
use std::sync::Mutex;

#[cfg(test)]
static TEST_BOOTSTRAP: Mutex<Option<TestBootstrapState>> = Mutex::new(None);

#[cfg(test)]
static TEST_RECONCILED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
pub(crate) fn take_reconciled_accounts() -> usize {
    TEST_RECONCILED.swap(0, std::sync::atomic::Ordering::SeqCst)
}

/// Resolved bootstrap values captured by generated-host integration tests.
#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TestBootstrapState {
    pub(crate) config_path: String,
    pub(crate) profiles: Vec<String>,
    pub(crate) log: String,
    pub(crate) log_format: LogFormat,
    pub(crate) color: ColorChoice,
}

#[cfg(test)]
pub(crate) fn take_test_bootstrap() -> TestBootstrapState {
    TEST_BOOTSTRAP
        .lock()
        .expect("test bootstrap capture lock is available")
        .take()
        .expect("generated host captured bootstrap state")
}

/// The accounts `after_build` reconciles before the application starts.
const HOUSEHOLD_ACCOUNTS: [u64; 3] = [1, 2, 3];

/// Bootstrap provenance retained for diagnostics throughout the host lifecycle.
pub struct StartupProvenance {
    pub(crate) config_path: String,
    pub(crate) profiles: Vec<String>,
    pub(crate) operator: String,
    pub(crate) audit_review_ticket: String,
}

/// Database readiness information produced after application construction.
pub struct BuildReadiness {
    pub(crate) schema_version: u64,
    pub(crate) reconciled_accounts: usize,
}

/// Captures generated bootstrap choices before tracing is finalized or a builder exists.
pub async fn setup(
    mut context: BootstrapContext,
) -> Result<BootstrapContext, std::convert::Infallible> {
    #[cfg(test)]
    if context.mode().is_run()
        && let Some(bootstrap) = context.bootstrap()
    {
        let snapshot = TestBootstrapState {
            config_path: bootstrap.config_path().display().to_string(),
            profiles: bootstrap.profiles().to_vec(),
            log: bootstrap.logging().level.clone(),
            log_format: bootstrap.logging().format,
            color: bootstrap.color(),
        };

        *TEST_BOOTSTRAP
            .lock()
            .expect("test bootstrap capture lock is available") = Some(snapshot);
    }

    let config_path = context
        .bootstrap()
        .map(|bootstrap| bootstrap.config_path().display().to_string())
        .unwrap_or_else(|| String::from("<direct-host>"));
    let profiles = context
        .bootstrap()
        .map(|bootstrap| bootstrap.profiles().to_vec())
        .unwrap_or_default();
    let operator = context
        .get::<OperationsArgs>()
        .map(|arguments| arguments.operator.clone())
        .unwrap_or_else(|| String::from("direct-host"));
    let audit_review_ticket = context
        .get::<ComplianceAuditArgs>()
        .map(|arguments| arguments.audit_review_ticket.clone())
        .unwrap_or_else(|| String::from("direct-host"));

    let provenance = StartupProvenance {
        config_path,
        profiles,
        operator,
        audit_review_ticket,
    };

    tracing::debug!(
        target: "homeledger::lifecycle",
        config_path = %provenance.config_path,
        profiles = ?provenance.profiles,
        operator = %provenance.operator,
        audit_review_ticket = %provenance.audit_review_ticket,
        "Homeledger bootstrap choices captured"
    );
    context.insert(provenance);

    Ok(context)
}

/// Verifies database readiness after DI and the RPC runtime have been constructed.
pub async fn after_build(
    context: &mut BootstrapContext,
    app: App<HomeledgerRpc>,
) -> Result<App<HomeledgerRpc>, DiError> {
    let database =
        resolve_host_dependency::<HomeledgerRpc, Database>(&app, "Homeledger after_build").await?;
    let schema_version = database.verify_schema();
    let runs = app
        .runtime()
        .opener::<AccountReconcile>()
        .map_err(|error| DiError::Other(Box::new(error)))?;
    let reconciled_accounts = reconcile_accounts(runs, HOUSEHOLD_ACCOUNTS)
        .await
        .map_err(|error| DiError::Other(Box::new(error)))?;

    #[cfg(test)]
    TEST_RECONCILED.store(reconciled_accounts, std::sync::atomic::Ordering::SeqCst);

    let readiness = BuildReadiness {
        schema_version,
        reconciled_accounts,
    };

    tracing::info!(
        target: "homeledger::lifecycle",
        schema_version = readiness.schema_version,
        reconciled_accounts = readiness.reconciled_accounts,
        "Homeledger database schema verified"
    );
    context.insert(readiness);

    Ok(app)
}
