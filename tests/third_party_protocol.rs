use std::cell::Cell;

use upwell::{
    AppError, AppRegistry, AppRuntime, PreparedProtocol, ProtocolDefinition, ProtocolId,
    ProtocolRuntime, Scope, ScopeBoundary, ScopeId, ScopeParent, ScopeTopology, Serve,
    ShutdownSignal, ValidationContext, app,
};

/// A third-party session boundary declared entirely through facade APIs.
pub struct SessionScope;

impl Scope for SessionScope {
    const ID: ScopeId = upwell::namespaced_id!(ScopeId, "third-party/session");
    const RANK: u8 = 200;
    const NAME: &'static str = "Session";
}

const SESSION_BOUNDARIES: [ScopeBoundary; 1] =
    [ScopeBoundary::new::<SessionScope>(&[ScopeParent::Root])];

/// A third-party protocol definition using only the public facade.
#[derive(Default)]
pub struct ThirdPartyProtocol;

/// Validated third-party state that is movable but intentionally not shareable.
pub struct PreparedThirdPartyProtocol {
    marker: Cell<u8>,
}

/// Built third-party protocol runtime.
pub struct ThirdPartyRuntime {
    marker: Cell<u8>,
}

impl ProtocolDefinition for ThirdPartyProtocol {
    type Prepared = PreparedThirdPartyProtocol;
    type Error = AppError;

    const ID: ProtocolId = upwell::namespaced_id!(ProtocolId, "third-party/example-protocol");
    const SCOPE_TOPOLOGY: ScopeTopology = ScopeTopology::new(&SESSION_BOUNDARIES);

    fn register(&self, _registry: &mut AppRegistry) {}

    fn prepare(self, context: &ValidationContext<'_>) -> Result<Self::Prepared, Self::Error> {
        assert_eq!(context.name(), "third-party-protocol-test");

        Ok(PreparedThirdPartyProtocol {
            marker: Cell::new(1),
        })
    }
}

impl PreparedProtocol for PreparedThirdPartyProtocol {
    type Runtime = ThirdPartyRuntime;
    type Error = AppError;

    fn build(self, _runtime: &AppRuntime) -> Result<Self::Runtime, Self::Error> {
        assert_eq!(self.marker.get(), 1);

        Ok(ThirdPartyRuntime {
            marker: Cell::new(2),
        })
    }

    #[cfg(feature = "tooling")]
    fn tooling(&self, contributions: &mut upwell_app::ToolingContributions) {
        contributions.display(upwell_app::ResourceDisplay {
            label: Some(String::from("Third-party protocol")),
            ..Default::default()
        });
    }
}

impl ProtocolRuntime for ThirdPartyRuntime {
    type Error = AppError;
}

impl Serve<()> for ThirdPartyRuntime {
    async fn serve(
        self,
        _runtime: AppRuntime,
        _shutdown: ShutdownSignal,
        _endpoint: (),
    ) -> Result<(), Self::Error> {
        assert_eq!(self.marker.get(), 2);

        Ok(())
    }
}

app! {
    app ThirdPartyApplication {
        name: "third-party-protocol-test",
        protocol: ThirdPartyProtocol,
    }
}

#[tokio::test]
async fn facade_supports_third_party_protocol_states_and_topology() {
    let prepared = ThirdPartyApplication::new(upwell::ExecutionMode::Run)
        .prepare()
        .await
        .expect("third-party application prepares");

    assert_eq!(prepared.app().scope_topology().boundaries().len(), 1);
    assert_eq!(prepared.app().protocol().marker.get(), 1);
    assert_eq!(
        prepared.app().protocol_id(),
        ThirdPartyProtocol::ID,
        "the prepared application retains authoritative protocol identity"
    );

    let built = prepared
        .build()
        .await
        .expect("third-party application builds");

    assert_eq!(built.app().protocol().marker.get(), 2);
    assert_eq!(built.app().protocol_id(), ThirdPartyProtocol::ID);

    let (_context, app) = built.into_parts();

    app.serve(())
        .await
        .expect("third-party runtime serves through facade contracts");
}
