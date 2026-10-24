//! Protocol-owned tooling metadata through the public prepared seam.

use super::*;

/// Third-party protocol definition using only the public prepared tooling seam.
#[derive(Default)]
struct ThirdPartyProtocol;

/// Prepared third-party protocol metadata.
struct PreparedThirdPartyProtocol;

/// Third-party protocol relying on the public prepared tooling default.
#[derive(Default)]
struct DefaultToolingProtocol;

/// Prepared third-party protocol with no tooling override.
struct PreparedDefaultToolingProtocol;

/// Built third-party protocol runtime.
struct ThirdPartyRuntime;

/// Protocol whose prepared tooling metadata is intentionally invalid.
#[derive(Default)]
struct InvalidToolingProtocol;

/// Prepared protocol emitting duplicate owner-local tooling identities.
struct InvalidPreparedToolingProtocol;

impl ProtocolDefinition for ThirdPartyProtocol {
    type Prepared = PreparedThirdPartyProtocol;
    type Error = crate::Error;

    const ID: crate::ProtocolId = crate::namespaced_id!(crate::ProtocolId, "third-party/protocol");
    const SCOPE_TOPOLOGY: ScopeTopology = ScopeTopology::empty();

    fn register(&self, _registry: &mut AppRegistry) {}

    fn prepare(self, _context: &ValidationContext<'_>) -> Result<Self::Prepared, Self::Error> {
        Ok(PreparedThirdPartyProtocol)
    }
}

impl ProtocolDefinition for DefaultToolingProtocol {
    type Prepared = PreparedDefaultToolingProtocol;
    type Error = crate::Error;

    const ID: crate::ProtocolId =
        crate::namespaced_id!(crate::ProtocolId, "third-party/default-tooling");
    const SCOPE_TOPOLOGY: ScopeTopology = ScopeTopology::empty();

    fn register(&self, _registry: &mut AppRegistry) {}

    fn prepare(self, _context: &ValidationContext<'_>) -> Result<Self::Prepared, Self::Error> {
        Ok(PreparedDefaultToolingProtocol)
    }
}

impl ProtocolDefinition for InvalidToolingProtocol {
    type Prepared = InvalidPreparedToolingProtocol;
    type Error = crate::Error;

    const ID: crate::ProtocolId = crate::namespaced_id!(crate::ProtocolId, "test/invalid-tooling");
    const SCOPE_TOPOLOGY: ScopeTopology = ScopeTopology::empty();

    fn register(&self, _registry: &mut AppRegistry) {}

    fn prepare(self, _context: &ValidationContext<'_>) -> Result<Self::Prepared, Self::Error> {
        Ok(InvalidPreparedToolingProtocol)
    }
}

impl PreparedProtocol for PreparedThirdPartyProtocol {
    type Runtime = ThirdPartyRuntime;
    type Error = crate::Error;

    fn build(self, _runtime: &crate::AppRuntime) -> Result<Self::Runtime, Self::Error> {
        Ok(ThirdPartyRuntime)
    }

    fn tooling(&self, contributions: &mut crate::ToolingContributions) {
        TOOLING_CALLS.fetch_add(1, Ordering::SeqCst);
        contributions.display(crate::ResourceDisplay {
            label: Some(String::from("Third-party protocol")),
            summary: Some(String::from("Third-party tooling contract")),
            ..crate::ResourceDisplay::default()
        });
        contributions.resource("transport", "Third-party transport");
        contributions.resource_display(
            "transport",
            crate::ResourceDisplay {
                label: Some(String::from("Third-party transport")),
                ..crate::ResourceDisplay::default()
            },
        );
        contributions.relationship(
            ToolingRelationshipKind::Contains,
            ToolingEndpoint::Owner,
            ToolingEndpoint::Resource("transport"),
        );
    }
}

impl PreparedProtocol for PreparedDefaultToolingProtocol {
    type Runtime = ThirdPartyRuntime;
    type Error = crate::Error;

    fn build(self, _runtime: &crate::AppRuntime) -> Result<Self::Runtime, Self::Error> {
        Ok(ThirdPartyRuntime)
    }
}

impl ProtocolRuntime for ThirdPartyRuntime {
    type Error = crate::Error;
}

impl PreparedProtocol for InvalidPreparedToolingProtocol {
    type Runtime = ThirdPartyRuntime;
    type Error = crate::Error;

    fn build(self, _runtime: &crate::AppRuntime) -> Result<Self::Runtime, Self::Error> {
        Ok(ThirdPartyRuntime)
    }

    fn tooling(&self, contributions: &mut crate::ToolingContributions) {
        contributions.display(crate::ResourceDisplay {
            label: Some(String::from("Invalid tooling protocol")),
            ..crate::ResourceDisplay::default()
        });
        contributions.resource("duplicate", "First");
        contributions.resource("duplicate", "Second");
    }
}

#[test]
fn third_party_protocol_without_tooling_override_uses_default_display() {
    let document = App::<DefaultToolingProtocol>::builder("default-tooling-protocol")
        .config_source(ConfigManager::<Toml>::empty())
        .prepare()
        .expect("third-party protocol prepares with default tooling")
        .tooling_document()
        .expect("default protocol tooling projects");
    let protocol = document
        .resources
        .iter()
        .find(|resource| resource.id == "protocol:third-party/default-tooling")
        .expect("protocol resource is projected");

    assert_eq!(
        protocol
            .display
            .as_ref()
            .and_then(|display| display.label.as_deref()),
        Some("Protocol")
    );
}

#[test]
fn third_party_protocol_projects_owner_scoped_generic_metadata() {
    TOOLING_CALLS.store(0, Ordering::SeqCst);

    let prepared = App::<ThirdPartyProtocol>::builder("third-party-protocol")
        .config_source(ConfigManager::<Toml>::empty())
        .prepare()
        .expect("third-party protocol prepares");
    let document = prepared
        .tooling_document()
        .expect("third-party metadata projects");

    prepared
        .tooling_document()
        .expect("third-party metadata reprojects");

    let protocol = document
        .resources
        .iter()
        .find(|resource| resource.id == "protocol:third-party/protocol")
        .expect("protocol resource is projected");

    assert_eq!(
        protocol
            .display
            .as_ref()
            .and_then(|display| display.label.as_deref()),
        Some("Third-party protocol")
    );
    assert!(document.resources.iter().any(|resource| {
        resource.id == "protocol:third-party/protocol/tooling/transport"
            && resource
                .provenance
                .as_ref()
                .and_then(|value| value.owner.as_deref())
                == Some("protocol:third-party/protocol")
    }));
    assert_eq!(TOOLING_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn invalid_protocol_tooling_is_a_typed_prepare_error() {
    let result = App::<InvalidToolingProtocol>::builder("invalid-tooling")
        .config_source(ConfigManager::<Toml>::empty())
        .prepare();
    let Err(error) = result else {
        panic!("invalid protocol tooling must fail during preparation");
    };

    assert!(matches!(
        error,
        crate::Error::ToolingContribution(
            crate::ToolingContributionError::DuplicateResource { .. }
        )
    ));
}
