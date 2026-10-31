//! Already open scopes versus future scope plans: a switch never migrates an open request
//! scope (it keeps the generation it pinned, nested openings included), while every scope
//! opened afterwards is built from the published generation's plan.

use std::sync::Arc;

use upwell_core::{Scope, ScopeId};
use upwell_di::{Component as _, ComponentDescriptor, EffectiveNodeRole, factories};

use super::super::fixture::{Authenticator, lock_test_guard};
use super::super::gate::GATE_COMPONENT;
use super::*;
use crate::test_support::test_component;
use crate::{
    AppRegistry, OpensUnder, ProtocolDefinition, ScopeBoundary, ScopeParent, ScopeTopology,
};

const CALL_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/hardening-call");
const STEP_ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/hardening-step");

/// A per-call scope opened under the root.
struct Call;

/// A nested step scope opened under a call.
struct Step;

impl Scope for Call {
    const ID: ScopeId = CALL_ID;
    const RANK: u8 = 50;
    const NAME: &'static str = "Call";
}

impl Scope for Step {
    const ID: ScopeId = STEP_ID;
    const RANK: u8 = 25;
    const NAME: &'static str = "Step";
}

impl OpensUnder<upwell_core::Singleton> for Call {}

impl OpensUnder<Call> for Step {}

static BOUNDARIES: [ScopeBoundary; 2] = [
    ScopeBoundary::new::<Call>(&[ScopeParent::Root]),
    ScopeBoundary::new::<Step>(&[ScopeParent::Boundary(CALL_ID)]),
];

/// A protocol declaring the call and step scopes.
#[derive(Default)]
struct ScopedProtocol;

impl ProtocolDefinition for ScopedProtocol {
    type Prepared = ();
    type Error = crate::Error;

    const ID: crate::ProtocolId = crate::namespaced_id!(crate::ProtocolId, "test/hardening");
    const SCOPE_TOPOLOGY: ScopeTopology = ScopeTopology::new(&BOUNDARIES);

    fn register(&self, _registry: &mut AppRegistry) {}

    fn prepare(self, _context: &crate::ValidationContext<'_>) -> crate::Result<Self::Prepared> {
        Ok(())
    }
}

/// A call-scoped consumer that captured the provider its call resolved.
struct CallGate {
    scheme: &'static str,
}

test_component!(CallGate, "hardening-call-gate");

async fn construct_call_gate(authenticator: Arc<dyn Authenticator>) -> CallGate {
    CallGate {
        scheme: authenticator.scheme(),
    }
}

static CALL_GATE_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<CallGate>()
    .with_scope(Call::KEY)
    .with_factories(factories!["static" => construct_call_gate]);

fn call_scheme(scope: &Arc<upwell_di::ScopeContainer>) -> &'static str {
    scope
        .get::<CallGate>()
        .expect("the call gate resolves in its call")
        .scheme
}

#[tokio::test]
async fn open_scopes_keep_their_generation_and_new_scopes_use_the_published_plan() {
    let _guard = lock_test_guard().await;
    let dir = auth_config_dir("primary");

    reset_faults();

    let app = auth_builder::<ScopedProtocol>(
        &config_dir_of(&dir),
        &PRIMARY_COMPONENT,
        &GATE_COMPONENT,
        &[&CALL_GATE_COMPONENT],
    )
    .build()
    .await
    .expect("the scoped app builds");
    let runtime = app.runtime();
    let calls = runtime
        .opener::<Call>()
        .expect("the call scope is declared");
    let steps = runtime
        .opener::<Step>()
        .expect("the step scope is declared");
    let open_call = calls
        .builder()
        .open_default()
        .await
        .expect("a call opens before the switch");

    write_auth_config(&config_dir_of(&dir), "fallback");

    let report = runtime
        .reload_config()
        .await
        .expect("a switch with an open call commits");
    let nested = steps
        .builder()
        .under(&open_call)
        .open_default()
        .await
        .expect("a step opens under the call opened before the switch");
    let new_call = calls
        .builder()
        .open_default()
        .await
        .expect("a call opens after the switch");
    let planned = report
        .component(CallGate::ID)
        .expect("the switch plans the call gate");

    assert!(report.is_committed());
    assert_eq!(planned.role, EffectiveNodeRole::FutureScope(Call::ID));
    assert_eq!(
        call_scheme(open_call.container()),
        "primary",
        "the open call is not migrated"
    );
    assert_eq!(
        nested
            .container()
            .get::<CallGate>()
            .expect("the step resolves its call's gate")
            .scheme,
        "primary",
        "a nested opening inherits its parent's pinned generation"
    );
    assert_eq!(
        call_scheme(new_call.container()),
        "fallback",
        "a new call uses the new plan"
    );
    assert_eq!(
        runtime.displaced_generations().len(),
        1,
        "the open call keeps the displaced generation alive"
    );

    drop((nested, open_call));

    assert!(
        runtime.displaced_generations().is_empty(),
        "closing the call releases the displaced generation"
    );
}

#[tokio::test]
async fn a_slot_built_on_demand_after_a_switch_uses_its_scopes_generation() {
    let _guard = lock_test_guard().await;
    let dir = auth_config_dir("primary");
    let key = upwell_core::EntryKey::new("test/hardening-call").expect("valid entry key");

    reset_faults();

    let app = auth_builder::<ScopedProtocol>(
        &config_dir_of(&dir),
        &PRIMARY_COMPONENT,
        &GATE_COMPONENT,
        &[&CALL_GATE_COMPONENT],
    )
    .entry_descriptor(crate::EntryDescriptor::new::<Call, ()>(key))
    .build()
    .await
    .expect("the scoped app builds");
    let runtime = app.runtime();
    let calls = runtime
        .opener::<Call>()
        .expect("the call scope is declared");
    let entry = calls.entry(key).expect("the call entry is registered");
    let open_call = calls
        .builder()
        .open(&entry)
        .await
        .expect("a call opens before the switch");
    let built_at_open = open_call.container().get::<CallGate>().is_some();

    write_auth_config(&config_dir_of(&dir), "fallback");

    runtime
        .reload_config()
        .await
        .expect("a switch with an open call commits");

    let gate = open_call
        .container()
        .resolve::<Arc<CallGate>>()
        .await
        .expect("resolution succeeds")
        .expect("the gate is built on demand");

    assert!(
        !built_at_open,
        "the entry injects nothing, so its call builds nothing"
    );
    assert_eq!(
        gate.scheme, "primary",
        "a slot built after the switch comes from the generation its call opened under"
    );
}
