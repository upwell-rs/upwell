//! Resources, facets, provenance, and the relationships between resources.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::SourceLocation;

/// Provenance shared by resources and diagnostics.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Provenance {
    /// Stable identity of the owning resource.
    pub owner: Option<String>,
    /// Semantic origin such as `protocol-default` or `application-configuration`.
    pub origin: Option<String>,
    /// Origin-local declaration order, when meaningful.
    pub ordinal: Option<u32>,
    /// Declaration source, when available.
    pub source: Option<SourceLocation>,
}

/// A namespaced opaque extension payload with its own additive schema version.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Facet {
    /// Facet-local schema version.
    pub schema_version: u16,
    /// Opaque JSON interpreted only by the facet owner.
    pub value: Value,
}

/// Generic resource categories understood without protocol knowledge.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum ResourceKind {
    /// A diagnostic-only identity whose generic category is not known.
    Unknown,
    /// The configured application.
    Application,
    /// The selected protocol definition.
    Protocol,
    /// An effective plugin implementation.
    Plugin,
    /// A component descriptor.
    Component,
    /// A trait provider descriptor.
    Provider,
    /// A safe config type/path binding.
    ConfigBinding,
    /// A hook descriptor.
    Hook,
    /// A lifecycle stage.
    Lifecycle,
    /// A prepared scope boundary.
    Scope,
    /// A stable Rust type name used as a graph endpoint.
    Type,
    /// A plugin contribution decision.
    Contribution,
    /// A framework, application, protocol, or plugin contribution owner.
    Contributor,
    /// A plugin capability slot, including an unoccupied relation endpoint.
    PluginSlot,
    /// One node of a component's condition expression.
    Condition,
    /// An entry point: a unit of work that runs inside a scope.
    Entry,
}

/// Classifies a diagnostic resource identity by its canonical generic prefix.
///
/// Producers may override this classification in [`ProbeFailure::resource_kinds`](crate::ProbeFailure::resource_kinds) when an
/// owner-qualified identity has more specific semantics.
pub fn diagnostic_resource_kind(id: &str) -> ResourceKind {
    let prefix = id.split_once(':').map_or(id, |(prefix, _)| prefix);

    match prefix {
        "application" => ResourceKind::Application,
        "protocol" => ResourceKind::Protocol,
        "plugin" => ResourceKind::Plugin,
        "component" => ResourceKind::Component,
        "provider" => ResourceKind::Provider,
        "config-binding" => ResourceKind::ConfigBinding,
        "hook" => ResourceKind::Hook,
        "lifecycle" => ResourceKind::Lifecycle,
        "scope" => ResourceKind::Scope,
        "type" => ResourceKind::Type,
        "contribution" => ResourceKind::Contribution,
        "plugin-slot" => ResourceKind::PluginSlot,
        "condition" => ResourceKind::Condition,
        "entry" => ResourceKind::Entry,
        "framework" => ResourceKind::Contributor,
        _ => ResourceKind::Unknown,
    }
}

/// Declarative owner-provided presentation for one resource.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceDisplay {
    /// Optional compact human-facing label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Optional owner-defined visual category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Optional one-line description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Deterministically keyed human-facing facts.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, String>,
}

impl ResourceDisplay {
    /// Returns whether this declaration contains no presentation information.
    pub fn is_empty(&self) -> bool {
        self.label.is_none()
            && self.group.is_none()
            && self.summary.is_none()
            && self.details.is_empty()
    }
}

/// One protocol-neutral inspectable resource.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Resource {
    /// Stable document-local identity.
    pub id: String,
    /// Generic resource category.
    pub kind: ResourceKind,
    /// Human-readable name.
    pub name: String,
    /// Declarative human presentation supplied by the resource owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<ResourceDisplay>,
    /// Stable declaration provenance, when available.
    #[serde(default)]
    pub provenance: Option<Provenance>,
    /// Deterministically ordered scalar metadata.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// Namespaced opaque extension data.
    #[serde(default)]
    pub facets: BTreeMap<String, Facet>,
}

impl Default for Resource {
    fn default() -> Self {
        Self {
            id: String::new(),
            kind: ResourceKind::Application,
            name: String::new(),
            display: None,
            provenance: None,
            labels: BTreeMap::new(),
            facets: BTreeMap::new(),
        }
    }
}

/// Typed generic relationship semantics.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum RelationshipKind {
    /// A resource requires another resource.
    DependsOn,
    /// A provider or component exposes a type.
    Provides,
    /// A config binding selects a type and safe property path.
    Binds,
    /// A hook belongs to a component or lifecycle stage.
    Hooks,
    /// A parent scope opens a child scope.
    OpensScope,
    /// A protocol, plugin, or owner-scoped contribution contains another contribution resource.
    Contains,
    /// A plugin emitted a retained contribution.
    Contributes,
    /// One plugin displaced another implementation.
    Replaces,
    /// A composition directive disabled a plugin implementation.
    Suppresses,
    /// The source precedes the target.
    OrdersBefore,
    /// The source follows the target.
    OrdersAfter,
    /// The source validated the target.
    Validates,
    /// The source cannot coexist with the target.
    Conflicts,
    /// A condition decides whether its target component, or parent condition, is eligible.
    Gates,
    /// An entry runs inside a scope.
    Enters,
}

/// One directed relationship between resources.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Relationship {
    /// Relationship semantics.
    pub kind: RelationshipKind,
    /// Stable source resource identity.
    pub from: String,
    /// Stable target resource identity.
    pub to: String,
    /// Deterministically ordered scalar edge metadata.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

impl Default for Relationship {
    fn default() -> Self {
        Self {
            kind: RelationshipKind::DependsOn,
            from: String::new(),
            to: String::new(),
            labels: BTreeMap::new(),
        }
    }
}
