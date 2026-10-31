use std::any::TypeId;
use std::collections::{BTreeSet, HashMap, HashSet};

use upwell_core::{DependencyDescriptor, Scope, ScopeId, Singleton, TypeDescriptor};
use upwell_di::{
    BoxedComponent, ComponentDescriptor, ComponentFactoryRole, ProviderSelectionModel,
    SelectedDependency,
};

use crate::entry::{EntryDescriptor, EntryTable};
use crate::scope::{PreparedScopeTopology, ScopeLayouts, SeedDestination};

/// Immutable callback-free input for prepared-state tooling projection.
pub(crate) struct ProjectionSnapshot {
    components: Vec<ComponentSnapshot>,
    candidates: Vec<CandidateSnapshot>,
    entries: Vec<EntrySnapshot>,
    root_plan: Vec<ConstructionPlanEntry>,
    scope_plans: HashMap<ScopeId, Vec<ConstructionPlanEntry>>,
}

impl ProjectionSnapshot {
    pub(crate) fn capture(
        selection: &ProviderSelectionModel,
        topology: &PreparedScopeTopology,
        descriptors: &[ComponentDescriptor],
        root_order: &[ComponentDescriptor],
        scope_orders: &HashMap<ScopeId, Vec<ComponentDescriptor>>,
        instances: &[BoxedComponent],
        seed_destinations: &HashMap<TypeId, SeedDestination>,
    ) -> upwell_di::Result<Self> {
        let seeded: HashSet<_> = instances
            .iter()
            .map(|instance| instance.ty.type_id)
            .collect();
        let components: Vec<_> = descriptors
            .iter()
            .map(|descriptor| {
                ComponentSnapshot::capture(
                    selection,
                    topology,
                    *descriptor,
                    &seeded,
                    seed_destinations,
                )
            })
            .collect::<upwell_di::Result<_>>()?;
        let root_ids: BTreeSet<_> = root_order
            .iter()
            .map(|component| component.ty.type_id)
            .collect();
        let mut root_plan: Vec<_> = components
            .iter()
            .filter(|component| component.descriptor.scope.id() == Singleton::ID)
            .filter(|component| component.seeded)
            .filter(|component| !root_ids.contains(&component.descriptor.ty.type_id))
            .map(ConstructionPlanEntry::from_component)
            .collect();

        root_plan.extend(root_order.iter().map(|descriptor| {
            ConstructionPlanEntry::from_component(component(&components, descriptor.ty.type_id))
        }));

        let scope_plans = scope_orders
            .iter()
            .map(|(scope, order)| {
                let entries = order
                    .iter()
                    .map(|descriptor| {
                        ConstructionPlanEntry::from_component(component(
                            &components,
                            descriptor.ty.type_id,
                        ))
                    })
                    .collect();

                (*scope, entries)
            })
            .collect();

        Ok(Self {
            components,
            candidates: Vec::new(),
            entries: Vec::new(),
            root_plan,
            scope_plans,
        })
    }

    /// Captures every candidate of the full condition `catalog`, eligible or not.
    pub(crate) fn with_candidates(mut self, catalog: &[ComponentDescriptor]) -> Self {
        self.candidates = catalog
            .iter()
            .map(|descriptor| CandidateSnapshot::capture(*descriptor))
            .collect();

        self
    }

    /// Captures every prepared entry with the selected target of each injected dependency.
    pub(crate) fn with_entries(
        mut self,
        selection: &ProviderSelectionModel,
        topology: &PreparedScopeTopology,
        entries: &EntryTable,
        layouts: &ScopeLayouts,
    ) -> upwell_di::Result<Self> {
        self.entries =
            entries
                .iter()
                .map(|(id, entry)| {
                    let builds = layouts.get(&entry.scope.id()).zip(layouts.entry(id)).map(
                        |(layout, plan)| {
                            plan.components(layout)
                                .map(|component| component.id)
                                .collect()
                        },
                    );

                    EntrySnapshot::capture(selection, topology, *entry, builds)
                })
                .collect::<upwell_di::Result<_>>()?;

        Ok(self)
    }

    pub(crate) fn components(&self) -> &[ComponentSnapshot] {
        &self.components
    }

    pub(crate) fn candidates(&self) -> &[CandidateSnapshot] {
        &self.candidates
    }

    pub(crate) fn entries(&self) -> &[EntrySnapshot] {
        &self.entries
    }

    pub(crate) fn root_plan(&self) -> &[ConstructionPlanEntry] {
        &self.root_plan
    }

    pub(crate) fn scope_plan(&self, scope: &ScopeId) -> Option<&[ConstructionPlanEntry]> {
        self.scope_plans.get(scope).map(Vec::as_slice)
    }
}

/// Callback-derived facts for one effective component descriptor.
#[derive(Clone)]
pub(crate) struct ComponentSnapshot {
    pub(crate) descriptor: ComponentDescriptor,
    pub(crate) has_factory: bool,
    pub(crate) factory_selection: FactorySelection,
    pub(crate) factory_candidate_count: usize,
    pub(crate) factory_explicit_count: usize,
    pub(crate) dependencies: Vec<DependencySnapshot>,
    pub(crate) hooks: Vec<HookSnapshot>,
    pub(crate) seeded: bool,
    pub(crate) seed_destination: Option<ScopeId>,
}

impl ComponentSnapshot {
    fn capture(
        selection: &ProviderSelectionModel,
        topology: &PreparedScopeTopology,
        descriptor: ComponentDescriptor,
        seeded: &HashSet<TypeId>,
        seed_destinations: &HashMap<TypeId, SeedDestination>,
    ) -> upwell_di::Result<Self> {
        let factories = (descriptor.factories)();
        let factory_explicit_count = factories
            .iter()
            .filter(|factory| factory.role == ComponentFactoryRole::Explicit)
            .count();
        let factory = descriptor
            .effective_factory()
            .expect("validated component factory remains unambiguous");
        let factory_selection = match factory {
            None => FactorySelection::Manual,
            Some(factory) if factory.role == ComponentFactoryRole::Default => {
                FactorySelection::Default
            }
            Some(_) => FactorySelection::Explicit,
        };
        let dependencies = factory.map_or_else(
            || Ok(Vec::new()),
            |factory| {
                (factory.dependencies)()
                    .into_iter()
                    .map(|dependency| {
                        DependencySnapshot::capture(selection, topology, descriptor, dependency)
                    })
                    .collect::<upwell_di::Result<_>>()
            },
        )?;
        let mut hooks: Vec<_> = (descriptor.hooks)()
            .iter()
            .copied()
            .map(|hook| HookSnapshot::capture(selection, topology, descriptor, hook))
            .collect::<upwell_di::Result<_>>()?;

        hooks.sort_by(|left, right| {
            left.kind
                .cmp(right.kind)
                .then_with(|| left.ordinal.cmp(&right.ordinal))
        });

        Ok(Self {
            descriptor,
            has_factory: factory.is_some(),
            factory_selection,
            factory_candidate_count: factories.len(),
            factory_explicit_count,
            dependencies,
            hooks,
            seeded: seeded.contains(&descriptor.ty.type_id),
            seed_destination: seed_destinations
                .get(&descriptor.ty.type_id)
                .map(|seed| seed.scope),
        })
    }
}

/// Callback-derived facts for one catalog candidate, whether or not its condition holds.
#[derive(Clone)]
pub(crate) struct CandidateSnapshot {
    pub(crate) descriptor: ComponentDescriptor,
    /// The transition strategies the descriptor supports, in preference order.
    pub(crate) transition_capabilities: Vec<&'static str>,
    /// The effective factory's declared dependencies, before any provider selection.
    pub(crate) dependencies: Vec<DependencyDescriptor>,
}

impl CandidateSnapshot {
    fn capture(descriptor: ComponentDescriptor) -> Self {
        let declared = |recipe: upwell_di::Result<Option<_>>| recipe.ok().flatten().is_some();
        let capabilities = [
            ("live-rebind", descriptor.exposes_live_cell()),
            ("hot-update", declared(descriptor.hot_update_factory())),
            (
                "transition-handler",
                declared(descriptor.transition_handler()),
            ),
            ("replacement", declared(descriptor.replacement_factory())),
            ("reconstruct", declared(descriptor.effective_factory())),
        ];

        Self {
            descriptor,
            transition_capabilities: capabilities
                .into_iter()
                .filter(|(_, supported)| *supported)
                .map(|(name, _)| name)
                .collect(),
            dependencies: descriptor.dependencies(),
        }
    }
}

/// Stable category of the effective component factory decision.
#[derive(Clone, Copy)]
pub(crate) enum FactorySelection {
    Manual,
    Default,
    Explicit,
}

impl FactorySelection {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Default => "default",
            Self::Explicit => "explicit",
        }
    }
}

/// One declared dependency and its producer-selected runtime targets.
#[derive(Clone)]
pub(crate) struct DependencySnapshot {
    pub(crate) descriptor: DependencyDescriptor,
    pub(crate) selected: Vec<SelectedDependency>,
}

impl DependencySnapshot {
    fn capture(
        selection: &ProviderSelectionModel,
        topology: &PreparedScopeTopology,
        consumer: ComponentDescriptor,
        descriptor: DependencyDescriptor,
    ) -> upwell_di::Result<Self> {
        let selected = selection.selected_dependencies_with_scope_reachability(
            &consumer,
            &descriptor,
            |consumer, dependency| topology.is_reachable(&consumer, &dependency),
        );

        Ok(Self {
            descriptor,
            selected,
        })
    }
}

/// Callback-derived facts for one prepared entry.
#[derive(Clone)]
pub(crate) struct EntrySnapshot {
    pub(crate) descriptor: EntryDescriptor,
    pub(crate) targets: Vec<DependencySnapshot>,
    /// The IDs of the components the entry builds when it opens its scope, in construction
    /// order. `None` for an entry into the root scope, which opens nothing.
    pub(crate) builds: Option<Vec<&'static str>>,
}

impl EntrySnapshot {
    /// Selects each target the way a component living in the entry's scope would: provider
    /// selection only reads the consumer's scope, so a factoryless stand-in named by the
    /// entry key carries it.
    fn capture(
        selection: &ProviderSelectionModel,
        topology: &PreparedScopeTopology,
        descriptor: EntryDescriptor,
        builds: Option<Vec<&'static str>>,
    ) -> upwell_di::Result<Self> {
        let key = descriptor.key.as_str();
        let consumer = ComponentDescriptor::manual(
            key,
            key,
            TypeDescriptor::of::<EntryDescriptor>("EntryDescriptor"),
            descriptor.scope,
        );

        Ok(Self {
            descriptor,
            targets: (descriptor.targets)()
                .into_iter()
                .map(|target| DependencySnapshot::capture(selection, topology, consumer, target))
                .collect::<upwell_di::Result<_>>()?,
            builds,
        })
    }
}

/// Callback-derived facts for one retained hook descriptor.
#[derive(Clone)]
pub(crate) struct HookSnapshot {
    pub(crate) ordinal: u32,
    pub(crate) kind: &'static str,
    pub(crate) source: Option<upwell_core::ItemSource>,
    pub(crate) dependencies: Vec<DependencySnapshot>,
}

impl HookSnapshot {
    fn capture(
        selection: &ProviderSelectionModel,
        topology: &PreparedScopeTopology,
        consumer: ComponentDescriptor,
        descriptor: upwell_hooks::HookDescriptor,
    ) -> upwell_di::Result<Self> {
        Ok(Self {
            ordinal: descriptor.ordinal,
            kind: descriptor.kind,
            source: descriptor.source,
            dependencies: (descriptor.dependencies)()
                .into_iter()
                .map(|dependency| {
                    DependencySnapshot::capture(selection, topology, consumer, dependency)
                })
                .collect::<upwell_di::Result<_>>()?,
        })
    }
}

/// One exact prepared construction-plan entry with its snapshotted selection outcome.
#[derive(Clone, Copy)]
pub(crate) struct ConstructionPlanEntry {
    pub(crate) descriptor: ComponentDescriptor,
    pub(crate) has_factory: bool,
}

impl ConstructionPlanEntry {
    fn from_component(component: &ComponentSnapshot) -> Self {
        Self {
            descriptor: component.descriptor,
            has_factory: component.has_factory,
        }
    }
}

fn component(components: &[ComponentSnapshot], ty: TypeId) -> &ComponentSnapshot {
    components
        .iter()
        .find(|component| component.descriptor.ty.type_id == ty)
        .expect("prepared construction plan references an effective component")
}
