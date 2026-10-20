//! Candidate root construction for a resolved transition plan.

use std::any::TypeId;
use std::collections::BTreeSet;
use std::sync::Arc;

use futures::FutureExt;
use upwell_core::ResolverSet;
use upwell_di::{
    BoxedComponent, ComponentChange, HotUpdateCommit, NodeAction, ScopeContainer, ScopeRegistry,
    TransitionPlan,
};

use super::{
    CandidateGraph, ComponentTransitionDecision, ComponentTransitionStrategy, RestartReason,
    RestartRequired,
};

/// Which snapshot of an active instance a decision's strategy needs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SeedKind {
    /// The instance itself carries into the candidate root.
    Retained,
    /// Candidate construction reads the instance through `Previous`.
    Previous,
}

impl SeedKind {
    fn seeds(self, strategy: ComponentTransitionStrategy) -> bool {
        match self {
            Self::Retained => strategy.retains_instance(),
            Self::Previous => strategy.reads_previous(),
        }
    }
}

/// Explicit execution choices validated against the graph planner's minimum requirements.
#[doc(hidden)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedTransitionPlan {
    pub(super) candidate_identity: Arc<()>,
    pub(super) active_identity: Arc<()>,
    pub(super) structural: TransitionPlan,
    pub(super) decisions: Box<[ComponentTransitionDecision]>,
    pub(super) construction_order: Box<[&'static str]>,
}

impl ResolvedTransitionPlan {
    pub fn structural(&self) -> &TransitionPlan {
        &self.structural
    }

    pub fn decisions(&self) -> &[ComponentTransitionDecision] {
        &self.decisions
    }

    pub fn construction_order(&self) -> &[&'static str] {
        &self.construction_order
    }

    /// Constructs the isolated candidate singleton root selected by this resolved plan.
    ///
    /// Retained instances (`Retain` and `HotUpdate` decisions) and the previous
    /// instances of `Replace` and `Transition` decisions must be supplied explicitly;
    /// prefer
    /// [`build_candidate_root_from_active`](Self::build_candidate_root_from_active),
    /// which derives both from the pinned active root.
    #[doc(hidden)]
    pub async fn build_candidate_root(
        &self,
        candidate: &CandidateGraph,
        retained: Vec<BoxedComponent>,
        previous: Vec<BoxedComponent>,
        externals: ResolverSet,
    ) -> crate::Result<(Arc<ScopeContainer>, Arc<ScopeRegistry>)> {
        if !Arc::ptr_eq(&self.candidate_identity, &candidate.identity) {
            return Err(upwell_di::StaleGraphCandidate {
                active: self.structural.base_generation,
                candidate: candidate.graph.generation(),
            }
            .into());
        }

        self.validate_seeds(candidate, SeedKind::Retained, &retained)?;
        self.validate_seeds(candidate, SeedKind::Previous, &previous)?;

        std::panic::AssertUnwindSafe(async {
            let transient = candidate
                .components
                .iter()
                .filter(|component| component.scope.is_transient())
                .map(|component| (component.ty.type_id, *component))
                .collect();
            let components = candidate
                .components
                .iter()
                .map(|component| (component.ty.type_id, *component))
                .collect();
            let scopes = Arc::new(ScopeRegistry::from_selection_model(
                transient,
                components,
                Arc::clone(candidate.provider_selection()),
            )?);
            let root = ScopeContainer::build_candidate_root(
                &self.construction_order,
                retained,
                previous,
                self.transition_changes(),
                externals,
                Arc::clone(&scopes),
            )
            .await?;

            Ok((root, scopes))
        })
        .catch_unwind()
        .await
        .map_err(|_| crate::Error::CandidatePreparationPanicked)?
    }

    /// Constructs the isolated candidate singleton root, deriving every retained
    /// instance, and the previous instance of every `Replace` decision, directly from
    /// the pinned active root.
    ///
    /// Each seed is snapshotted out of the active root's local store through its typed
    /// generation-snapshot adapter, so a replacement factory reads an immutable snapshot
    /// and the active instance stays intact for rollback. A raw manual descriptor
    /// carries no adapter, so it is rejected with
    /// [`RestartReason::ManualInstanceUnsupported`]. A typed handle that declines
    /// generation-local snapshots returns [`RestartReason::HandleRetentionUnsupported`];
    /// missing storage, panic, and type mismatch surface as DI errors.
    #[doc(hidden)]
    pub async fn build_candidate_root_from_active(
        &self,
        candidate: &CandidateGraph,
        active: &crate::RuntimeView,
        externals: ResolverSet,
    ) -> crate::Result<(Arc<ScopeContainer>, Arc<ScopeRegistry>)> {
        self.build_candidate_root_from_active_with_overrides(
            candidate,
            active,
            externals,
            Vec::new(),
        )
        .await
    }

    /// Constructs the isolated candidate singleton root like
    /// [`build_candidate_root_from_active`](Self::build_candidate_root_from_active),
    /// replacing select snapshot-derived seeds with caller-supplied generation
    /// overrides.
    ///
    /// `generation_overrides` replaces the snapshot-derived seed for a retained
    /// framework singleton that must be fresh in every generation (the
    /// `HookManager`). Each override must be unique and must target a component this
    /// plan retains; every other retained singleton still comes from the snapshot
    /// path, and the merged seed set must still satisfy the plan's complete retain
    /// dispositions.
    #[doc(hidden)]
    pub async fn build_candidate_root_from_active_with_overrides(
        &self,
        candidate: &CandidateGraph,
        active: &crate::RuntimeView,
        externals: ResolverSet,
        generation_overrides: Vec<BoxedComponent>,
    ) -> crate::Result<(Arc<ScopeContainer>, Arc<ScopeRegistry>)> {
        if !Arc::ptr_eq(&self.active_identity, active.effective_graph().identity()) {
            return Err(upwell_di::StaleGraphCandidate {
                active: self.structural.base_generation,
                candidate: active.id(),
            }
            .into());
        }

        self.validate_generation_overrides(candidate, &generation_overrides)?;

        let overridden = generation_overrides
            .iter()
            .map(|seed| seed.ty.type_id)
            .collect::<BTreeSet<_>>();
        let mut retained =
            self.snapshot_from_active(candidate, active.root(), SeedKind::Retained, &overridden)?;
        let previous = self.snapshot_from_active(
            candidate,
            active.root(),
            SeedKind::Previous,
            &BTreeSet::new(),
        )?;

        retained.extend(generation_overrides);

        self.build_candidate_root(candidate, retained, previous, externals)
            .await
    }

    /// Snapshots one stored representation per decision seeding `kind` out of the active
    /// root.
    ///
    /// Overridden types are skipped: their seed is supplied by the caller instead of
    /// the snapshot adapter, so snapshot safety is never bypassed for them.
    fn snapshot_from_active(
        &self,
        candidate: &CandidateGraph,
        active_root: &ScopeContainer,
        kind: SeedKind,
        overridden: &BTreeSet<TypeId>,
    ) -> crate::Result<Vec<BoxedComponent>> {
        let mut seeds = Vec::new();

        for decision in self.decisions_seeding(kind) {
            let Some(descriptor) = candidate.component(decision.component) else {
                continue;
            };

            if overridden.contains(&descriptor.ty.type_id) {
                continue;
            }

            let reason = match active_root.snapshot_singleton(*descriptor) {
                Ok(component) => {
                    seeds.push(component);

                    continue;
                }
                Err(upwell_di::Error::SnapshotUnavailable { .. }) => {
                    RestartReason::ManualInstanceUnsupported
                }
                Err(upwell_di::Error::SnapshotHandleUnsupported { .. }) => {
                    RestartReason::HandleRetentionUnsupported
                }
                Err(error) => return Err(error.into()),
            };

            return Err(self.restart(decision, reason).into());
        }

        Ok(seeds)
    }

    /// Validates generation override seeds against the resolved plan's retain dispositions.
    ///
    /// An override replaces the snapshot-derived retained instance for exactly one retained
    /// framework singleton. Overrides must be unique and must target a component the plan
    /// retains, so reconstruction and snapshot safety are never bypassed; the merged seed
    /// set is still completeness-checked by [`validate_seeds`](Self::validate_seeds) and
    /// the DI candidate input validation.
    fn validate_generation_overrides(
        &self,
        candidate: &CandidateGraph,
        overrides: &[BoxedComponent],
    ) -> crate::Result<()> {
        let retained = self.seed_types(candidate, SeedKind::Retained);
        let mut seen = BTreeSet::new();

        for seed in overrides {
            let type_name = seed.ty.name;

            if !seen.insert(seed.ty.type_id) {
                return Err(crate::Error::DuplicateGenerationOverride { type_name });
            }

            if !retained.contains(&seed.ty.type_id) {
                return Err(crate::Error::InvalidGenerationOverride { type_name });
            }
        }

        Ok(())
    }

    /// Requires `seeds` to cover exactly the components whose decision seeds `kind`.
    fn validate_seeds(
        &self,
        candidate: &CandidateGraph,
        kind: SeedKind,
        seeds: &[BoxedComponent],
    ) -> crate::Result<()> {
        let supplied = seeds
            .iter()
            .map(|component| component.ty.type_id)
            .collect::<BTreeSet<_>>();
        let expected = self.seed_types(candidate, kind);

        if supplied == expected {
            return Ok(());
        }

        let mismatched = self.decisions.iter().find(|decision| {
            candidate
                .component(decision.component)
                .is_some_and(|component| {
                    supplied.contains(&component.ty.type_id) != kind.seeds(decision.strategy)
                })
        });
        let error = match mismatched {
            Some(decision) => self.restart(decision, RestartReason::MissingDecision),

            None => RestartRequired {
                component: "<seed>",
                required: None,
                reason: RestartReason::MissingDecision,
            },
        };

        Err(error.into())
    }

    /// The planned change of every `Transition` decision, for its transition handler.
    fn transition_changes(&self) -> Vec<ComponentChange> {
        self.decisions
            .iter()
            .filter(|decision| decision.strategy == ComponentTransitionStrategy::Transition)
            .filter_map(|decision| {
                self.structural
                    .nodes
                    .iter()
                    .find(|node| node.component == decision.component)
            })
            .map(|node| ComponentChange::new(node.clone()))
            .collect()
    }

    /// Prepares the hot update of every `HotUpdate` decision against the built candidate
    /// root, in decision order. Nothing is applied until the returned commits run.
    pub(crate) async fn prepare_hot_updates(
        &self,
        candidate: &CandidateGraph,
        candidate_root: &Arc<ScopeContainer>,
    ) -> crate::Result<Vec<HotUpdateCommit>> {
        let mut commits = Vec::new();

        for decision in self
            .decisions
            .iter()
            .filter(|decision| decision.strategy == ComponentTransitionStrategy::HotUpdate)
        {
            let descriptor = candidate
                .component(decision.component)
                .ok_or_else(|| self.restart(decision, RestartReason::HotUpdateUnavailable))?;

            commits.push(candidate_root.prepare_hot_update(*descriptor).await?);
        }

        Ok(commits)
    }

    /// The singletons this plan displaces, in retirement order: replaced or removed
    /// components whose decision does not keep the active instance.
    pub fn displaced(&self) -> Vec<&'static str> {
        self.structural
            .retirement_order
            .iter()
            .filter(|component| {
                !self.decisions.iter().any(|decision| {
                    decision.component == **component && decision.strategy.retains_instance()
                })
            })
            .copied()
            .collect()
    }

    /// The decisions whose strategy needs a `kind` seed from the active root.
    pub(super) fn decisions_seeding(
        &self,
        kind: SeedKind,
    ) -> impl Iterator<Item = &ComponentTransitionDecision> {
        self.decisions
            .iter()
            .filter(move |decision| kind.seeds(decision.strategy))
    }

    fn seed_types(&self, candidate: &CandidateGraph, kind: SeedKind) -> BTreeSet<TypeId> {
        self.decisions_seeding(kind)
            .filter_map(|decision| candidate.component(decision.component))
            .map(|component| component.ty.type_id)
            .collect()
    }

    /// A restart requirement for `decision`, naming the structural action it was meant
    /// to satisfy. A retained component outside the structural plan requires `Retain`.
    pub(super) fn restart(
        &self,
        decision: &ComponentTransitionDecision,
        reason: RestartReason,
    ) -> RestartRequired {
        let required = self
            .structural
            .nodes
            .iter()
            .find(|node| node.component == decision.component)
            .map_or(Some(NodeAction::Retain), |node| Some(node.action));

        RestartRequired {
            component: decision.component,
            required,
            reason,
        }
    }
}
