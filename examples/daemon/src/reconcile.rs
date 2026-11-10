//! A user-authored scope: reconciling one household account, opened from the `after_build` hook.
//!
//! [`AccountReconcile`] is declared with `#[scope]`, so discovery adds it to the RPC topology.
//! Its field is the parameter of each instance, and [`AccountLedger`] injects the instance's
//! value as `Arc<AccountReconcile>`. Each run gets its own ledger, so parallel runs share
//! nothing scoped; what they do share, the database, is a singleton.

use std::sync::Arc;

use tokio::task::JoinSet;
use upwell::scope::Root;
use upwell::{AppError, ScopeOpener, component, scope};

use crate::components::Database;

/// One account's reconciliation run.
#[scope(parents = [Root])]
pub struct AccountReconcile {
    account: u64,
}

/// The ledger of the account its [`AccountReconcile`] instance reconciles.
#[component(scope = AccountReconcile)]
pub struct AccountLedger {
    run: Arc<AccountReconcile>,
    database: Database,
}

impl AccountLedger {
    /// Reconciles the account against the database's schema, returning the account.
    fn reconcile(&self) -> u64 {
        tracing::debug!(
            target: "homeledger::reconcile",
            account = self.run.account,
            schema_version = self.database.verify_schema(),
            "Homeledger account reconciled"
        );

        self.run.account
    }
}

/// Reconciles every account in `accounts` in parallel, each in its own scope instance, and
/// returns how many finished.
pub async fn reconcile_accounts(
    runs: ScopeOpener<AccountReconcile>,
    accounts: impl IntoIterator<Item = u64>,
) -> Result<usize, AppError> {
    let mut tasks = JoinSet::new();
    let mut reconciled = 0;

    for account in accounts {
        let runs = runs.clone();

        tasks.spawn(async move {
            runs.builder(AccountReconcile { account })
                .try_run(|ledger: Arc<AccountLedger>| async move {
                    Ok::<_, std::convert::Infallible>(ledger.reconcile())
                })
                .await
                .map_err(AppError::from)
        });
    }

    while let Some(run) = tasks.join_next().await {
        run.expect("a reconcile run does not panic")?;
        reconciled += 1;
    }

    Ok(reconciled)
}
