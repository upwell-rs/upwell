//! Read-only projection of prepared application plans into the tooling schema.

use thiserror::Error;
use upwell_di::ProviderDescriptor;
use upwell_tooling_schema::Provenance;

use crate::{ContributionProvenance, Contributor, InstallationOrigin, InstallationProvenance};

mod composition;
mod config_schema;
mod contribution;
mod failure;
mod output;
mod panic;
mod preview;
mod probe;
mod projection;
mod relationship;
mod resources;
mod snapshot;
mod source;

pub(crate) use contribution::ToolingContributionSet;
pub use contribution::{
    ToolingContributionError, ToolingContributions, ToolingEndpoint, ToolingRelationshipKind,
};
pub use output::{
    ToolingProbeOutputError, ToolingProbeOutputTargetError, emit_probe_envelope,
    emit_probe_envelope_from_env,
};
pub use panic::install_process_probe_panic_hook;
#[cfg(feature = "cli")]
pub use probe::probe_bootstrapped_host;
pub use probe::{catch_probe_panic, probe_host, probe_target_identity_from_env};
use projection::Projection;
pub(crate) use snapshot::ProjectionSnapshot;
pub(crate) use source::{descriptor_location, item_location, type_location, with_source};
pub use upwell_tooling_schema::ResourceDisplay;

/// A typed failure while projecting an already prepared application.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ToolingProjectionError {
    /// A prepared protocol or plugin supplied invalid owner-scoped metadata.
    #[error(transparent)]
    Contribution(#[from] ToolingContributionError),
}

/// A typed failure while resolving the selected target for a generated tooling entry.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ToolingProbeTargetError {
    /// A required invoker-owned environment variable is absent.
    #[error("required tooling probe environment variable '{name}' is not set")]
    MissingEnvironment {
        /// Missing variable name.
        name: &'static str,
    },
    /// An invoker-owned environment variable is not valid Unicode.
    #[error("tooling probe environment variable '{name}' is not valid Unicode")]
    InvalidEnvironment {
        /// Invalid variable name.
        name: &'static str,
    },
    /// An invoker-owned environment variable is empty or contains only whitespace.
    #[error("tooling probe environment variable '{name}' is empty")]
    EmptyEnvironment {
        /// Empty variable name.
        name: &'static str,
    },
    /// The Cargo manifest path is not an absolute path.
    #[error("tooling probe manifest path must be absolute")]
    RelativeManifestPath,
    /// The invoker-owned package or binary target identity is incomplete.
    #[error(transparent)]
    InvalidIdentity(#[from] upwell_tooling_schema::IdentityValidationError),
}

fn component_resource(component: &str) -> String {
    format!("component:{component}")
}

fn config_resource(path: &str) -> String {
    format!("config:{path}")
}

fn installation_provenance(provenance: InstallationProvenance) -> Provenance {
    let (owner, origin) = match provenance.origin() {
        InstallationOrigin::ProtocolMandatory(protocol) => (
            Some(format!("protocol:{}", protocol.as_str())),
            "protocol-mandatory",
        ),
        InstallationOrigin::ProtocolDefault(protocol) => (
            Some(format!("protocol:{}", protocol.as_str())),
            "protocol-default",
        ),
        InstallationOrigin::ApplicationDeclaration => {
            (Some(String::from("application")), "application-declaration")
        }
        InstallationOrigin::ApplicationConfiguration => (
            Some(String::from("application")),
            "application-configuration",
        ),
    };

    Provenance {
        owner,
        origin: Some(origin.to_string()),
        ordinal: Some(provenance.ordinal()),
        ..Provenance::default()
    }
}

fn contribution_provenance(provenance: ContributionProvenance) -> Provenance {
    Provenance {
        owner: Some(contributor_id(provenance.contributor())),
        origin: Some(String::from("contribution")),
        ..Provenance::default()
    }
}

fn contributor_id(contributor: Contributor) -> String {
    match contributor {
        Contributor::Framework => String::from("framework"),
        Contributor::Application => String::from("application"),
        Contributor::Protocol(protocol) => format!("protocol:{}", protocol.as_str()),
        Contributor::Plugin(plugin) => plugin_id(plugin.as_str()),
    }
}

fn contribution_id(provenance: ContributionProvenance) -> String {
    upwell_tooling_schema::contribution_id(
        &contributor_id(provenance.contributor()),
        provenance.contribution().as_str(),
    )
}

fn lifecycle_resource(kind: &str) -> Option<&'static str> {
    match kind {
        "startup" => Some("lifecycle:startup"),
        "shutdown" => Some("lifecycle:shutdown"),
        "config_reload" => Some("lifecycle:config_reload"),
        _ => None,
    }
}

fn component_id(id: &str) -> String {
    format!("component:{id}")
}

fn provider_id(provider: &ProviderDescriptor) -> String {
    format!(
        "provider:{}:{}:{}",
        (provider.trait_ty.type_name)(),
        (provider.concrete_ty.type_name)(),
        provider.qualifier
    )
}

fn config_binding_id(rust_type: &str, path: &str) -> String {
    format!("config-binding:{rust_type}:{path}")
}

fn plugin_id(id: &str) -> String {
    format!("plugin:{id}")
}

fn scope_id(id: &str) -> String {
    format!("scope:{id}")
}

fn type_id(rust_type: &str) -> String {
    format!("type:{rust_type}")
}

#[cfg(test)]
mod tests;
