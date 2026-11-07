//! One instance of an application's own scope, opened for an entry through its
//! [`ScopeOpener`], as code running a unit of work in it does.

use std::sync::Arc;

use upwell_app::{
    App, AppRegistry, AppRuntime, EntryDescriptor, OpensUnder, PreparedProtocol,
    ProtocolDefinition, ProtocolId, ProtocolRuntime, ScopeBoundary, ScopeEntry, ScopeOpener,
    ScopeParent, ScopeTopology, ValidationContext,
};
use upwell_config::{ConfigManager, Dynamic};
use upwell_core::{EntryKey, Scope, ScopeId, Singleton};

use crate::alloc::Delta;
use crate::roster::{Scoped, UNUSED, per_call, roster, runtime};

/// One customer's synchronisation run: a scope the application declares and opens itself.
struct CustomerSync;

impl Scope for CustomerSync {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "contract/customer-sync");
    const RANK: u8 = 200;
    const NAME: &'static str = "CustomerSync";
}

impl OpensUnder<Singleton> for CustomerSync {}

const BOUNDARIES: [ScopeBoundary; 1] = [ScopeBoundary::new::<CustomerSync>(&[ScopeParent::Root])];

/// A protocol declaring [`CustomerSync`] and nothing else.
#[derive(Default)]
struct Syncs;

/// [`Syncs`] once prepared and built: it serves nothing.
struct Prepared;

impl ProtocolDefinition for Syncs {
    type Prepared = Prepared;
    type Error = upwell_app::Error;

    const ID: ProtocolId = upwell_core::namespaced_id!(ProtocolId, "contract/syncs");
    const SCOPE_TOPOLOGY: ScopeTopology = ScopeTopology::new(&BOUNDARIES);

    fn register(&self, _registry: &mut AppRegistry) {}

    fn prepare(self, _context: &ValidationContext<'_>) -> Result<Self::Prepared, Self::Error> {
        Ok(Prepared)
    }
}

impl PreparedProtocol for Prepared {
    type Runtime = Prepared;
    type Error = upwell_app::Error;

    fn build(self, _runtime: &AppRuntime) -> Result<Self::Runtime, Self::Error> {
        Ok(Prepared)
    }
}

impl ProtocolRuntime for Prepared {
    type Error = upwell_app::Error;
}

/// The run's entry: what it injects.
#[derive(Clone, Copy, PartialEq)]
enum Run {
    /// Nothing.
    Empty,
    /// `Arc<Scoped<0>>`, a component of the run's scope.
    Scoped,
}

impl Run {
    fn key(self) -> EntryKey {
        let key = match self {
            Self::Empty => "app/contract-empty-run",
            Self::Scoped => "app/contract-scoped-run",
        };

        EntryKey::new(key).expect("contract entry keys are valid")
    }

    fn entry(self) -> EntryDescriptor {
        match self {
            Self::Empty => EntryDescriptor::new::<CustomerSync, ()>(self.key()),
            Self::Scoped => EntryDescriptor::new::<CustomerSync, (Arc<Scoped<0>>,)>(self.key()),
        }
    }
}

/// An app with `count` components in [`CustomerSync`] and the opener of runs of `run`.
struct Runs {
    _app: App<Syncs>,
    opener: ScopeOpener<CustomerSync>,
    entry: ScopeEntry<CustomerSync>,
}

impl Runs {
    async fn build(count: usize, run: Run) -> Self {
        let builder = App::<Syncs>::builder("opener-allocation-contracts")
            .config_source(ConfigManager::<Dynamic>::empty())
            .entry_descriptor(run.entry());
        let app = roster(CustomerSync::KEY, count)
            .iter()
            .fold(builder, |builder, component| {
                builder.component_descriptor(component)
            })
            .build()
            .await
            .expect("the contract app builds");
        let opener = app
            .runtime()
            .opener::<CustomerSync>()
            .expect("the run's scope is declared");
        let entry = opener.entry(run.key()).expect("the run's entry resolves");

        Self {
            _app: app,
            opener,
            entry,
        }
    }

    async fn run(&self, run: Run) -> bool {
        let scope = self
            .opener
            .builder(CustomerSync)
            .open(&self.entry)
            .await
            .expect("the run's scope opens");

        match run {
            Run::Empty => true,
            Run::Scoped => scope
                .container()
                .extract_planned::<Arc<Scoped<0>>>(None)
                .await
                .is_ok(),
        }
    }
}

/// What opening one run of `run` allocates in an app with `count` components in its scope.
fn measured(count: usize, run: Run) -> Delta {
    let runtime = runtime();
    let runs = runtime.block_on(Runs::build(count, run));
    per_call(&runtime, || runs.run(run))
}

#[test]
fn unused_components_cost_an_opening_nothing() {
    let without = measured(0, Run::Empty);
    let with_unused = measured(UNUSED, Run::Empty);

    assert_eq!(with_unused, without);
}

#[test]
fn an_opening_injecting_one_component_costs_its_plan_not_the_scope() {
    let alone = measured(1, Run::Scoped);
    let among_unused = measured(1 + UNUSED, Run::Scoped);

    assert_eq!(
        among_unused, alone,
        "{UNUSED} unused components changed what opening a run injecting one allocates"
    );
}
