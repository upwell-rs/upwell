#![cfg(feature = "daemon")]

//! End-to-end tests for application-declared `#[scope]` types: their place in the topology,
//! their parameters as injectable values, and instances opened through `ScopeOpener`.
//!
//! [`CustomerSync`] is a parameterised scope under the root. Its components read the instance's
//! parameters and count their builds, so a test reads back which instance built what.

extern crate upwell as fork;

mod access;
mod declaration;
mod isolation;
mod opener;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use upwell::daemon::App;
use upwell::scope::Root;
use upwell::{ScopeOpener, component, entry, methods, scope};

/// Counts [`Unused`] builds across every test of this binary.
static UNUSED_BUILDS: AtomicUsize = AtomicUsize::new(0);

/// One customer's synchronisation run.
#[scope(parents = [Root])]
pub struct CustomerSync {
    customer: u64,
}

/// One batch of a [`CustomerSync`] run, a unit scope below it.
#[scope(parents = [CustomerSync], generations = durable)]
pub struct Batch;

/// The client of the customer its [`CustomerSync`] instance syncs.
#[component(scope = CustomerSync)]
struct CustomerClient {
    sync: Arc<CustomerSync>,
}

/// Records what [`CustomerClient`] its instance shares.
#[component(scope = CustomerSync)]
struct SyncAudit {
    client: Arc<CustomerClient>,
}

/// A component of the [`Batch`] scope below [`CustomerSync`].
#[component(scope = Batch)]
struct BatchStep {}

/// An entry into [`CustomerSync`], planned at preparation rather than per run.
#[entry(scope = CustomerSync)]
async fn reconcile(_client: Arc<CustomerClient>) {}

/// A component of the scope that no run injects.
#[component(scope = CustomerSync)]
struct Unused {
    #[default]
    _build: UnusedBuild,
}

/// Counts one [`Unused`] build.
struct UnusedBuild;

impl Default for UnusedBuild {
    fn default() -> Self {
        UNUSED_BUILDS.fetch_add(1, Ordering::SeqCst);

        Self
    }
}

/// A singleton holding the injected opener of [`CustomerSync`].
#[component(default_factory = false)]
struct Syncer {
    sync: ScopeOpener<CustomerSync>,
}

#[methods]
impl Syncer {
    #[init]
    fn new(sync: ScopeOpener<CustomerSync>) -> Self {
        Self { sync }
    }
}

impl Syncer {
    /// Syncs `customer` in a fresh instance and returns the customer its client saw.
    async fn sync(&self, customer: u64) -> Result<u64, upwell::AppError> {
        self.sync
            .builder(CustomerSync { customer })
            .run(|client: Arc<CustomerClient>| async move { client.customer() })
            .await
    }
}

impl CustomerClient {
    fn customer(&self) -> u64 {
        self.sync.customer
    }
}

/// A built application with every `#[scope]` and component of this binary discovered.
async fn app(name: &str) -> App {
    App::builder(name)
        .auto_discover()
        .build()
        .await
        .expect("the application builds")
}

/// The opener of [`CustomerSync`] on `app`'s runtime.
fn opener(app: &App) -> ScopeOpener<CustomerSync> {
    app.runtime()
        .opener::<CustomerSync>()
        .expect("discovery declares the customer sync scope")
}
