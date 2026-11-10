use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use upwell_config::{ConfigManager, Toml};
use upwell_config::{ConfigProperties, ConfigReload};
use upwell_core::{Cardinality, DependencyDescriptor, ResolutionMode, Scope, TypeDescriptor};
use upwell_di::{
    BoxedComponent, Component, ComponentConstructionContext, ComponentDescriptor,
    ComponentFactoryDescriptor, ComponentFactoryRole, Injectable, ProviderDescriptor,
    ProviderOrder, ProviderOrderDirection, Singleton,
};
use upwell_tooling_schema::{
    BinaryTargetIdentity, DocumentIdentity, PackageIdentity, ProbeEnvelope, RelationshipKind,
    SourceLocation,
};

use super::emit_probe_envelope;
use super::probe::ToolingProbeError;
use crate::test_support::{TempFixture, test_component};
use crate::{
    App, AppRegistry, CompositionDiagnostic, CompositionDiagnostics, InstallationOrigin,
    InstallationProvenance, Plugin, PluginContributions, PluginId, PluginRelation, PluginSlotId,
    PreparedProtocol, ProtocolDefinition, ProtocolPluginRegistrar, ProtocolRuntime, RelationKind,
    RelationTarget, ScopeBoundary, ScopeParent, ScopeTopology, ToolingEndpoint,
    ToolingRelationshipKind, ValidationContext,
};

static FACTORY_CALLS: AtomicUsize = AtomicUsize::new(0);
static TOOLING_CALLS: AtomicUsize = AtomicUsize::new(0);
static FACTORY_DESCRIPTOR_CALLS: AtomicUsize = AtomicUsize::new(0);
static DEPENDENCY_DESCRIPTOR_CALLS: AtomicUsize = AtomicUsize::new(0);
static HOOK_DESCRIPTOR_CALLS: AtomicUsize = AtomicUsize::new(0);
static HOOK_DEPENDENCY_CALLS: AtomicUsize = AtomicUsize::new(0);
static PROJECTION_CALLBACK_POISONED: AtomicBool = AtomicBool::new(false);

fn probe_identity() -> DocumentIdentity {
    DocumentIdentity {
        application: String::from("tooling-tests"),
        package: Some(PackageIdentity {
            name: String::from("upwell-app"),
            version: Some(String::from(env!("CARGO_PKG_VERSION"))),
            manifest_path: Some(format!("{}/Cargo.toml", env!("CARGO_MANIFEST_DIR"))),
        }),
        binary: Some(BinaryTargetIdentity {
            name: String::from("tooling-tests"),
        }),
        source: Some(SourceLocation {
            file: String::from(file!()),
            line: None,
            column: None,
            rust_path: None,
        }),
        workspace_root: None,
    }
}

use failure_fixtures::*;
use fixtures::*;
use plugins::*;

mod conditions;
mod config_projection;
mod failure_fixtures;
mod failures;
mod fixtures;
mod plugins;
mod projection;
mod protocol;
