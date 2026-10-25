use std::cell::Cell;

use upwell_app::{
    App, AppRegistry, AppRuntime, PreparedProtocol, ProtocolDefinition, ProtocolId,
    ProtocolRuntime, Scope, ScopeBoundary, ScopeId, ScopeParent, ScopeTopology, Serve,
    ShutdownSignal, ValidationContext,
};

/// A custom boundary authored through `upwell-app` alone.
struct SessionScope;

impl Scope for SessionScope {
    const ID: ScopeId = upwell_app::namespaced_id!(ScopeId, "third-party/session");
    const RANK: u8 = 200;
    const NAME: &'static str = "Session";
}

const BOUNDARIES: [ScopeBoundary; 1] = [ScopeBoundary::new::<SessionScope>(&[ScopeParent::Root])];

/// A protocol definition authored through `upwell-app` alone.
#[derive(Default)]
struct Definition;

/// Its validated state.
struct Prepared(Cell<u8>);

/// Its built runtime.
struct Runtime(Cell<u8>);

impl ProtocolDefinition for Definition {
    type Prepared = Prepared;
    type Error = upwell_app::Error;

    const ID: ProtocolId =
        upwell_app::namespaced_id!(ProtocolId, "third-party/direct-app-contract");
    const SCOPE_TOPOLOGY: ScopeTopology = ScopeTopology::new(&BOUNDARIES);

    fn register(&self, _registry: &mut AppRegistry) {}

    fn prepare(self, _context: &ValidationContext<'_>) -> Result<Self::Prepared, Self::Error> {
        Ok(Prepared(Cell::new(1)))
    }
}

impl PreparedProtocol for Prepared {
    type Runtime = Runtime;
    type Error = upwell_app::Error;

    fn build(self, _runtime: &AppRuntime) -> Result<Self::Runtime, Self::Error> {
        assert_eq!(self.0.get(), 1);

        Ok(Runtime(Cell::new(2)))
    }

    #[cfg(feature = "tooling")]
    fn tooling(&self, contributions: &mut upwell_app::ToolingContributions) {
        contributions.display(upwell_app::ResourceDisplay {
            label: Some(String::from("Direct app test protocol")),
            ..Default::default()
        });
    }
}

impl ProtocolRuntime for Runtime {
    type Error = upwell_app::Error;
}

impl Serve<()> for Runtime {
    async fn serve(
        self,
        _runtime: AppRuntime,
        _shutdown: ShutdownSignal,
        _endpoint: (),
    ) -> Result<(), Self::Error> {
        assert_eq!(self.0.get(), 2);

        Ok(())
    }
}

#[tokio::test]
async fn direct_crate_exports_support_third_party_protocols() {
    assert_eq!(Definition::ID.as_str(), "third-party/direct-app-contract");
    assert_eq!(Definition::SCOPE_TOPOLOGY.boundaries().len(), 1);

    let prepared = App::<Definition>::builder("direct-third-party-protocol")
        .prepare()
        .expect("third-party definition prepares");

    assert_eq!(prepared.protocol().0.get(), 1);

    let app = prepared
        .build()
        .await
        .expect("third-party protocol runtime builds");

    assert_eq!(app.protocol().0.get(), 2);

    app.serve(())
        .await
        .expect("third-party protocol runtime serves");
}
