#![cfg(feature = "tracing-subscriber")]

use std::sync::Arc;

use futures::StreamExt;
use tracing::dispatcher::DefaultGuard;
use tracing::level_filters::LevelFilter;
use tracing::{Dispatch, Instrument};
use tracing_subscriber::Registry;
use tracing_subscriber::layer::SubscriberExt;
use upwell_core::{LogParams, Scope, ScopeId};

use super::*;

mod routing;
mod streaming;

const CUSTOMER_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/customer");

/// A scope with one loggable field and one that must stay out of logs.
struct Customer {
    id: u64,
    token: &'static str,
}

impl Scope for Customer {
    const ID: ScopeId = CUSTOMER_ID;
    const RANK: u8 = 10;
    const NAME: &'static str = "Customer";

    fn record_log_params(&self, params: &mut dyn LogParams) {
        params.param("id", &self.id);
    }
}

fn customer(id: u64) -> Customer {
    Customer {
        id,
        token: "secret",
    }
}

fn options() -> LogOptions {
    LogOptions {
        capture: true,
        ..LogOptions::new()
    }
}

/// Installs a subscriber holding the scope log layer for the current thread.
fn install() -> DefaultGuard {
    tracing::dispatcher::set_default(&Dispatch::new(
        Registry::default().with(ScopeLogLayer::new()),
    ))
}

/// Opens an instance of `id` that captures, with a subscriber already on its sink.
fn instance(id: u64, options: &LogOptions) -> (InstanceLogs, ScopeLogStream) {
    let logs = InstanceLogs::open(&customer(id), options, None);
    let stream = logs.sink().expect("the instance captures").subscribe();

    (logs, stream)
}

/// Every record of `stream`, once its instance finished.
async fn records(stream: ScopeLogStream) -> Vec<LogRecord> {
    stream
        .filter_map(|item| async move {
            match item {
                ScopeLog::Record(record) => Some(record),
                _ => None,
            }
        })
        .collect()
        .await
}

fn messages(records: &[LogRecord]) -> Vec<&str> {
    records
        .iter()
        .map(|record| record.message.as_str())
        .collect()
}
