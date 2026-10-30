use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

use upwell_core::{
    Cardinality, DependencyDescriptor, ResolutionMode, Scope, ScopeId, ScopeKey, Singleton,
};

use super::{
    DepRef, PlannedDependency, ProvidedRef, ProviderSlot, RootTarget, SlotIdx, SlotLayout, SlotRef,
    SlotSpec, TransientPlan,
};
use crate::descriptors::{ComponentDescriptor, ComponentFactoryDescriptor, ProviderDescriptor};
use crate::error::Error;
use crate::registry::{DependencyTarget, ProviderSelectionModel};

impl SlotLayout {
    /// Plans the slot layout of `scope`.
    ///
    /// `seeded` are the scope's factory-less components and `order` its factory-backed ones
    /// in construction order. `ancestors` holds the already planned layouts of the scopes
    /// open around it, and `can_reach` is the scope reachability validation used, so every
    /// edge resolves to the source validation selected.
    pub fn plan(
        scope: ScopeKey,
        seeded: &[ComponentDescriptor],
        order: &[ComponentDescriptor],
        ancestors: &HashMap<ScopeId, Arc<SlotLayout>>,
        selection: &ProviderSelectionModel,
        can_reach: impl Fn(ScopeId, ScopeId) -> bool,
    ) -> crate::Result<Self> {
        let mut layout = Self::index(scope, seeded, order, selection)?;
        let planner = Planner {
            layout: &layout,
            ancestors,
            selection,
            can_reach: &can_reach,
        };
        let mut dependencies = Vec::with_capacity(layout.slots.len());

        for spec in layout.slots.iter() {
            let planned = match spec.factory {
                Some(factory) => {
                    planner.factory_dependencies(&spec.component, factory, &mut Vec::new())?
                }
                None => Box::default(),
            };

            dependencies.push(planned);
        }

        for (spec, planned) in layout.slots.iter_mut().zip(dependencies) {
            spec.dependencies = planned;
        }

        Ok(layout)
    }

    /// Lays out the slots, seeds first, and builds the type and trait indexes.
    fn index(
        scope: ScopeKey,
        seeded: &[ComponentDescriptor],
        order: &[ComponentDescriptor],
        selection: &ProviderSelectionModel,
    ) -> crate::Result<Self> {
        let mut slots = Vec::with_capacity(seeded.len() + order.len());
        let mut by_type = HashMap::with_capacity(slots.capacity());
        let mut by_trait: HashMap<TypeId, Vec<ProviderSlot>> = HashMap::new();

        for component in seeded {
            slots.push(slot_spec(*component, None, selection));
        }

        for component in order {
            slots.push(slot_spec(
                *component,
                component.effective_factory()?,
                selection,
            ));
        }

        for (index, spec) in slots.iter().enumerate() {
            let slot = SlotIdx::new(index);

            by_type.insert(spec.component.ty.type_id, slot);

            for (alias, descriptor) in spec.providers.iter().enumerate() {
                let at = SlotRef {
                    slot,
                    alias: Some(alias),
                };

                by_trait
                    .entry(descriptor.trait_ty.type_id)
                    .or_default()
                    .push(ProviderSlot {
                        at,
                        descriptor: *descriptor,
                    });
            }
        }

        let by_trait = by_trait
            .into_iter()
            .map(|(trait_id, mut providers)| {
                providers.sort_by_key(|provider| selection.ordinal(&provider.descriptor));

                (trait_id, providers.into_boxed_slice())
            })
            .collect();

        let constructs = slots.iter().any(|spec| spec.factory.is_some());

        Ok(Self {
            scope,
            slots: slots.into_boxed_slice(),
            by_type,
            by_trait,
            constructs,
        })
    }
}

fn slot_spec(
    component: ComponentDescriptor,
    factory: Option<&'static ComponentFactoryDescriptor>,
    selection: &ProviderSelectionModel,
) -> SlotSpec {
    SlotSpec {
        component,
        factory,
        dependencies: Box::default(),
        providers: selection
            .providers_for_concrete(component.ty.type_id)
            .into(),
    }
}

/// Whether an edge is resolved by construction itself. Config, dynamic, lazy, deferred
/// and fresh edges resolve through their own handles, so they keep the dynamic path.
fn is_planned(dependency: &DependencyDescriptor) -> bool {
    !dependency.config && !dependency.dynamic && dependency.resolution == ResolutionMode::Eager
}

/// Resolves the dependency edges of one scope's factories against its own slots and the
/// layouts of the scopes open around it.
struct Planner<'a, R> {
    layout: &'a SlotLayout,
    ancestors: &'a HashMap<ScopeId, Arc<SlotLayout>>,
    selection: &'a ProviderSelectionModel,
    can_reach: &'a R,
}

impl<R: Fn(ScopeId, ScopeId) -> bool> Planner<'_, R> {
    /// Plans every edge of `factory` at its declared position, so a generated factory reads
    /// its edges by index. `visiting` holds the transients being planned, so a transient
    /// cycle is reported instead of recursing forever.
    fn factory_dependencies(
        &self,
        consumer: &ComponentDescriptor,
        factory: &ComponentFactoryDescriptor,
        visiting: &mut Vec<TypeId>,
    ) -> crate::Result<Box<[PlannedDependency]>> {
        let dependencies = (factory.dependencies)();
        let mut planned = Vec::with_capacity(dependencies.len());

        for dependency in dependencies {
            let source = if is_planned(&dependency) {
                self.source(consumer, &dependency, visiting)?
            } else {
                DepRef::Unplanned
            };

            planned.push(PlannedDependency {
                target: dependency.ty.type_id,
                qualifier: dependency.qualifier,
                cardinality: dependency.cardinality,
                source,
            });
        }

        Ok(planned.into_boxed_slice())
    }

    fn source(
        &self,
        consumer: &ComponentDescriptor,
        dependency: &DependencyDescriptor,
        visiting: &mut Vec<TypeId>,
    ) -> crate::Result<DepRef> {
        let selected = self
            .selection
            .selected_dependencies_with_scope_reachability(consumer, dependency, |from, to| {
                (self.can_reach)(from, to)
            });

        if dependency.cardinality != Cardinality::One {
            let mut provided = Vec::with_capacity(selected.len());

            for target in selected {
                let qualifier = match target.target {
                    DependencyTarget::Provider(provider) => provider.qualifier,
                    DependencyTarget::Component(component) => component.id,
                };
                let source = self.locate(consumer, dependency, target.target, visiting)?;

                provided.push(ProvidedRef { qualifier, source });
            }

            return Ok(DepRef::Many(provided.into_boxed_slice()));
        }

        match selected.first() {
            Some(target) => self.locate(consumer, dependency, target.target, visiting),

            None if dependency.optional => Ok(DepRef::Absent),

            None => Err(missing_dependency(consumer, dependency)),
        }
    }

    /// The source holding `target` as seen from the scope being planned.
    fn locate(
        &self,
        consumer: &ComponentDescriptor,
        dependency: &DependencyDescriptor,
        target: DependencyTarget,
        visiting: &mut Vec<TypeId>,
    ) -> crate::Result<DepRef> {
        let (component, provider) = match target {
            DependencyTarget::Component(component) => (component, None),
            DependencyTarget::Provider(provider) => (
                self.selection
                    .component(provider.concrete_ty.type_id)
                    .ok_or(Error::MissingComponent(provider.concrete_ty.name))?,
                Some(provider),
            ),
        };
        let scope = component.scope;

        if scope.id() == Singleton::ID {
            let root = match provider {
                Some(provider) => RootTarget::Provider(Box::new(provider)),
                None => RootTarget::Component(component.ty.type_id),
            };

            return Ok(DepRef::Root(root));
        }

        if scope.is_transient() {
            let plan = self.transient(component, provider, visiting)?;

            return Ok(DepRef::Transient(Arc::new(plan)));
        }

        let unplaced = || unplaced_dependency(consumer, dependency, &component);

        if scope.id() == self.layout.scope.id() {
            let at = slot_ref(self.layout, component, provider.as_ref()).ok_or_else(unplaced)?;

            return Ok(DepRef::Local(at));
        }

        let layout = self.ancestors.get(&scope.id()).ok_or_else(unplaced)?;
        let at = slot_ref(layout, component, provider.as_ref()).ok_or_else(unplaced)?;

        Ok(DepRef::Ancestor {
            layout: Arc::clone(layout),
            at,
        })
    }

    fn transient(
        &self,
        component: ComponentDescriptor,
        provider: Option<ProviderDescriptor>,
        visiting: &mut Vec<TypeId>,
    ) -> crate::Result<TransientPlan> {
        let type_id = component.ty.type_id;

        if visiting.contains(&type_id) {
            return Err(Error::DependencyCycle(component.name.to_string()));
        }

        let factory = component
            .effective_factory()?
            .ok_or(Error::MissingComponent(component.name))?;

        visiting.push(type_id);

        let dependencies = self.factory_dependencies(&component, factory, visiting);

        visiting.pop();

        Ok(TransientPlan {
            factory,
            provider,
            dependencies: dependencies?,
        })
    }
}

/// The slot, and provider alias when `provider` is set, that holds `component` in `layout`.
fn slot_ref(
    layout: &SlotLayout,
    component: ComponentDescriptor,
    provider: Option<&ProviderDescriptor>,
) -> Option<SlotRef> {
    let slot = layout.slot_of(component.ty.type_id)?;
    let alias = match provider {
        Some(provider) => Some(layout.spec(slot).alias_of(provider)?),
        None => None,
    };

    Some(SlotRef { slot, alias })
}

fn missing_dependency(consumer: &ComponentDescriptor, dependency: &DependencyDescriptor) -> Error {
    Error::MissingDependency {
        component: consumer.name.to_string(),
        component_id: consumer.id.to_string(),
        dependency: dependency.name.to_string(),
        type_name: (dependency.ty.type_name)().to_string(),
    }
}

fn unplaced_dependency(
    consumer: &ComponentDescriptor,
    dependency: &DependencyDescriptor,
    target: &ComponentDescriptor,
) -> Error {
    Error::UnplacedDependency {
        component_id: consumer.id.to_string(),
        dependency: dependency.name.to_string(),
        target: target.id.to_string(),
        scope: target.scope.name(),
    }
}
