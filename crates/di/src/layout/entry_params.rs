//! The sources an entry's injected targets resolve to, pre-resolved with its plan so reading a
//! built target costs a scan of the entry's few parameters and a handle clone.

use std::any::TypeId;

use upwell_core::{
    Cardinality, DependencyDescriptor, ResolutionMode, Scope, ScopeId, ScopeKey, Singleton,
};

use super::{DepRef, RootTarget, SlotLayout, SlotRef};
use crate::registry::ProviderSelectionModel;

/// One target an entry injects and the source it resolves to from the entry's scope.
///
/// Only sources a plan carried into a later generation still names correctly are recorded: a
/// slot of the entry's own scope (a carried plan's layout holds the same slots) and a root
/// singleton (a carried plan's targets select the same providers). A target in an enclosing
/// scope, a transient, an ambiguous or unsatisfied trait and anything resolved through its own
/// handle has no parameter and is extracted as any other value. `shadowed_by` lists the scopes
/// that make the chain select something else, as for the layout's selected sources.
#[derive(Clone, Debug)]
pub(crate) struct EntryParam {
    pub(crate) target: TypeId,
    pub(crate) qualifier: Option<&'static str>,
    pub(crate) cardinality: Cardinality,
    pub(crate) source: DepRef,
    pub(crate) shadowed_by: Box<[ScopeKey]>,
}

impl EntryParam {
    /// Whether this parameter is the target `target` injected with `qualifier` and
    /// `cardinality`.
    pub(crate) fn is(
        &self,
        target: TypeId,
        qualifier: Option<&str>,
        cardinality: Cardinality,
    ) -> bool {
        self.target == target && self.qualifier == qualifier && self.cardinality == cardinality
    }
}

impl PartialEq for EntryParam {
    fn eq(&self, other: &Self) -> bool {
        self.is(other.target, other.qualifier, other.cardinality)
            && self.shadowed_by == other.shadowed_by
            && same_source(&self.source, &other.source)
    }
}

impl Eq for EntryParam {}

impl SlotLayout {
    /// The parameters of an entry injecting `targets`, for every target whose source a
    /// parameter can record (see [`EntryParam`]), in target order. Repeated targets keep
    /// their first parameter.
    pub(super) fn plan_params(
        &self,
        targets: &[DependencyDescriptor],
        selection: &ProviderSelectionModel,
    ) -> Box<[EntryParam]> {
        let mut params: Vec<EntryParam> = Vec::new();

        for target in targets.iter().filter(|target| is_parameter(target)) {
            if params
                .iter()
                .any(|param| param.is(target.ty.type_id, target.qualifier, target.cardinality))
            {
                continue;
            }

            params.extend(self.param(selection, target));
        }

        params.into_boxed_slice()
    }

    /// The parameter of `dependency`, when a parameter can record its source.
    fn param(
        &self,
        selection: &ProviderSelectionModel,
        dependency: &DependencyDescriptor,
    ) -> Option<EntryParam> {
        let (target, qualifier, cardinality) = (
            dependency.ty.type_id,
            dependency.qualifier,
            dependency.cardinality,
        );
        let param = |source, shadowed_by| EntryParam {
            target,
            qualifier,
            cardinality,
            source,
            shadowed_by,
        };

        if !selection.providers_for_trait(target).is_empty() {
            return match cardinality {
                Cardinality::One => {
                    let selected = self.selected(target, qualifier)?;

                    recorded(&selected.source)
                        .then(|| param(selected.source.clone(), selected.shadowed_by.clone()))
                }

                Cardinality::Collection | Cardinality::Keyed => {
                    let selected = self.selected_many(target, cardinality)?;

                    selected
                        .providers
                        .iter()
                        .all(|provided| recorded(&provided.source))
                        .then(|| {
                            param(
                                DepRef::Many(selected.providers.clone()),
                                selected.shadowed_by.clone(),
                            )
                        })
                }
            };
        }

        if cardinality != Cardinality::One || qualifier.is_some() {
            return None;
        }

        let placements = selection.placements(target);
        let in_scope = |scope: ScopeId| {
            placements
                .iter()
                .any(|component| component.scope.id() == scope)
        };
        let source = if placements.is_empty() {
            if !dependency.optional {
                return None;
            }

            DepRef::Absent
        } else if in_scope(self.scope.id()) {
            DepRef::Local(SlotRef {
                slot: self.slot_of(target)?,
                alias: None,
            })
        } else if in_scope(Singleton::ID) {
            DepRef::Root(RootTarget::Component(target))
        } else {
            return None;
        };

        Some(param(source, Box::default()))
    }
}

/// Whether `target` is injected eagerly as a component, so its value can be read built.
fn is_parameter(target: &DependencyDescriptor) -> bool {
    !target.config && !target.dynamic && target.resolution == ResolutionMode::Eager
}

/// Whether a parameter can record `source` (see [`EntryParam`]).
fn recorded(source: &DepRef) -> bool {
    matches!(source, DepRef::Local(_) | DepRef::Root(_))
}

/// Whether two recorded sources name the same instance.
fn same_source(left: &DepRef, right: &DepRef) -> bool {
    match (left, right) {
        (DepRef::Local(left), DepRef::Local(right)) => left == right,

        (DepRef::Root(left), DepRef::Root(right)) => same_root(left, right),

        (DepRef::Many(left), DepRef::Many(right)) => {
            left.len() == right.len()
                && left.iter().zip(right.iter()).all(|(left, right)| {
                    left.qualifier == right.qualifier && same_source(&left.source, &right.source)
                })
        }

        (DepRef::Absent, DepRef::Absent) => true,

        _ => false,
    }
}

fn same_root(left: &RootTarget, right: &RootTarget) -> bool {
    match (left, right) {
        (RootTarget::Component(left), RootTarget::Component(right)) => left == right,

        (RootTarget::Provider(left), RootTarget::Provider(right)) => {
            left.trait_ty.type_id == right.trait_ty.type_id
                && left.concrete_ty.type_id == right.concrete_ty.type_id
                && left.qualifier == right.qualifier
        }

        _ => false,
    }
}
