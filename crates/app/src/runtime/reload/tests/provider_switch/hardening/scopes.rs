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
use crate::{AppRegistry, ProtocolDefinition, ScopeBoundary, ScopeParent, ScopeTopology};

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
    let open_call = runtime
        .open_scope_from_root(Call::KEY, Vec::new())
        .await
        .expect("a call opens before the switch");

    write_auth_config(&config_dir_of(&dir), "fallback");

    let report = runtime
        .reload_config()
        .await
        .expect("a switch with an open call commits");
    let nested = runtime
        .open_scope(Step::KEY, Arc::clone(&open_call), Vec::new())
        .await
        .expect("a step opens under the call opened before the switch");
    let new_call = runtime
        .open_scope_from_root(Call::KEY, Vec::new())
        .await
        .expect("a call opens after the switch");
    let planned = report
        .component(CallGate::ID)
        .expect("the switch plans the call gate");

    assert!(report.is_committed());
    assert_eq!(planned.role, EffectiveNodeRole::FutureScope(Call::ID));
    assert_eq!(
        call_scheme(&open_call),
        "primary",
        "the open call is not migrated"
    );
    assert_eq!(
        nested
            .get::<CallGate>()
            .expect("the step resolves its call's gate")
            .scheme,
        "primary",
        "a nested opening inherits its parent's pinned generation"
    );
    assert_eq!(
        call_scheme(&new_call),
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
