use std::any::TypeId;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use upwell_config::ConfigReloader;
use upwell_di::ProviderSelectionModel;

use crate::app::{HostLifecycleCapabilities, PreparedApp};
use crate::error::Error;
use crate::host::ExecutionMode;
use crate::protocol::{PreparedProtocol, ProtocolDefinition};
use crate::registry::AppRegistry;
use crate::runtime::AppConditionState;
use crate::tooling::{ProjectionSnapshot, ToolingContributionSet, ToolingContributions};

/// The prepared-state facts tooling projection reads, captured during preparation and
/// dropped by [`PreparedApp::build`].
pub(crate) struct PreparedTooling {
    mode: ExecutionMode,
    host_lifecycle: Option<HostLifecycleCapabilities>,
    provider_order: HashMap<TypeId, HashMap<TypeId, usize>>,
    protocol_tooling: ToolingContributionSet,
    config_sources: Arc<[PathBuf]>,
    snapshot: Arc<ProjectionSnapshot>,
}

impl PreparedTooling {
    /// Captures the protocol's tooling contributions, the config sources, each provider's
    /// selection ordinal, and the projection snapshot extended with the full candidate catalog.
    pub(super) fn capture<D: ProtocolDefinition>(
        mode: ExecutionMode,
        protocol: &D::Prepared,
        reloader: &ConfigReloader,
        registry: &AppRegistry,
        condition: &AppConditionState,
        selection: &ProviderSelectionModel,
        snapshot: ProjectionSnapshot,
    ) -> crate::Result<Self> {
        let mut contributions = ToolingContributions::new(format!("protocol:{}", D::ID.as_str()));

        protocol.tooling(&mut contributions);

        let provider_order =
            registry
                .providers
                .iter()
                .fold(HashMap::new(), |mut order, provider| {
                    order
                        .entry(provider.trait_ty.type_id)
                        .or_insert_with(HashMap::new)
                        .insert(provider.concrete_ty.type_id, selection.ordinal(provider));

                    order
                });

        Ok(Self {
            mode,
            host_lifecycle: None,
            provider_order,
            protocol_tooling: contributions.finish().map_err(Error::from)?,
            config_sources: Arc::from(reloader.sources()),
            snapshot: Arc::new(snapshot.with_candidates(&condition.catalog().components)),
        })
    }
}

impl<D: ProtocolDefinition> PreparedApp<D> {
    pub(crate) fn retain_host_lifecycle(&mut self, capabilities: HostLifecycleCapabilities) {
        self.tooling.host_lifecycle = Some(capabilities);
    }

    pub(crate) const fn host_lifecycle(&self) -> Option<HostLifecycleCapabilities> {
        self.tooling.host_lifecycle
    }

    pub(crate) fn provider_order(&self, trait_ty: TypeId, concrete_ty: TypeId) -> Option<usize> {
        self.tooling
            .provider_order
            .get(&trait_ty)?
            .get(&concrete_ty)
            .copied()
    }

    pub(crate) const fn protocol_tooling(&self) -> &ToolingContributionSet {
        &self.tooling.protocol_tooling
    }

    pub(crate) fn config_sources(&self) -> &[PathBuf] {
        &self.tooling.config_sources
    }

    pub(crate) fn tooling_snapshot(&self) -> &ProjectionSnapshot {
        &self.tooling.snapshot
    }

    pub(crate) const fn execution_mode(&self) -> ExecutionMode {
        self.tooling.mode
    }

    /// The full candidate catalog and the condition evaluation this plan was prepared from.
    pub(crate) fn condition_state(&self) -> &AppConditionState {
        &self.condition
    }

    pub(crate) fn config_reloader(&self) -> &ConfigReloader {
        &self.reloader
    }

    pub(crate) fn effective_graph(&self) -> &upwell_di::EffectiveGraph {
        &self.effective_graph
    }

    pub(crate) fn component_resource_id(&self, ty: TypeId) -> Option<String> {
        self.registry
            .components
            .iter()
            .find(|component| component.ty.type_id == ty)
            .map(|component| format!("component:{}", component.id))
    }
}
