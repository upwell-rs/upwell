//! Validation of candidate construction inputs and factory outputs.

use std::any::TypeId;
use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};

use upwell_core::{Singleton, StaticScope};

use crate::container::ScopeRegistry;
use crate::{BoxedComponent, ComponentChange, ComponentDescriptor, Error, ROOT_RESOLVER_ID};

pub(super) fn validate_candidate_inputs(
    order: &[&'static str],
    retained: &[BoxedComponent],
    registry: &ScopeRegistry,
) -> crate::Result<Vec<ComponentDescriptor>> {
    let mut types = HashSet::new();

    for component in retained {
        let Some(descriptor) = registry.component(component.ty.type_id) else {
            return Err(Error::CandidateComponentMismatch {
                component: component.ty.name,
            });
        };

        if descriptor.id == ROOT_RESOLVER_ID {
            return Err(Error::CandidateRuntimeBoundComponent {
                component: descriptor.id,
            });
        }

        if descriptor.scope.id() != Singleton::ID || !types.insert(component.ty.type_id) {
            return Err(Error::CandidateComponentMismatch {
                component: descriptor.id,
            });
        }
    }

    let mut descriptors = Vec::with_capacity(order.len());

    for id in order {
        let Some(descriptor) = registry.component_by_id(id) else {
            return Err(Error::CandidateComponentMismatch { component: id });
        };

        if descriptor.scope.id() != Singleton::ID || !types.insert(descriptor.ty.type_id) {
            return Err(Error::CandidateComponentMismatch {
                component: descriptor.id,
            });
        }

        descriptors.push(descriptor);
    }

    let expected = registry
        .components()
        .filter(|component| component.scope.id() == Singleton::ID)
        .filter(|component| component.id != ROOT_RESOLVER_ID)
        .map(|component| component.ty.type_id)
        .collect::<HashSet<_>>();

    if types != expected {
        let component = registry
            .components()
            .filter(|component| component.scope.id() == Singleton::ID)
            .filter(|component| component.id != ROOT_RESOLVER_ID)
            .find(|component| !types.contains(&component.ty.type_id))
            .map_or("<duplicate>", |component| component.id);

        return Err(Error::CandidateComponentMismatch { component });
    }

    Ok(descriptors)
}

/// Keys planned changes by component, accepting only one per component in `order` whose
/// descriptor declares a transition handler.
pub(super) fn validate_change_inputs(
    order: &[ComponentDescriptor],
    changes: Vec<ComponentChange>,
) -> crate::Result<HashMap<&'static str, ComponentChange>> {
    let mut keyed = HashMap::with_capacity(changes.len());

    for change in changes {
        let component = change.component();
        let accepted = match order.iter().find(|descriptor| descriptor.id == component) {
            Some(descriptor) => descriptor.transition_handler()?.is_some(),

            None => false,
        };

        if !accepted || keyed.insert(component, change).is_some() {
            return Err(Error::CandidateComponentMismatch { component });
        }
    }

    Ok(keyed)
}

/// Keys previous snapshots by type, accepting only one per component in `order` whose
/// descriptor declares a replacement factory or a transition handler.
pub(super) fn validate_previous_inputs(
    order: &[ComponentDescriptor],
    previous: Vec<BoxedComponent>,
) -> crate::Result<HashMap<TypeId, BoxedComponent>> {
    let mut keyed = HashMap::with_capacity(previous.len());

    for component in previous {
        let type_id = component.ty.type_id;
        let replaceable = order
            .iter()
            .find(|descriptor| descriptor.ty.type_id == type_id);
        let accepted = match replaceable {
            Some(descriptor) => {
                descriptor.replacement_factory()?.is_some()
                    || descriptor.transition_handler()?.is_some()
            }

            None => false,
        };

        if !accepted || keyed.insert(type_id, component).is_some() {
            return Err(Error::CandidateComponentMismatch {
                component: replaceable.map_or("<previous>", |descriptor| descriptor.id),
            });
        }
    }

    Ok(keyed)
}

pub(super) fn validate_factory_output(
    descriptor: ComponentDescriptor,
    component: &BoxedComponent,
) -> crate::Result<()> {
    catch_unwind(AssertUnwindSafe(|| {
        if component.ty.type_id == descriptor.ty.type_id {
            return Ok(());
        }

        Err(Error::CandidateFactoryTypeMismatch {
            component: descriptor.id,
            expected: (descriptor.ty.type_name)(),
            actual: (component.ty.type_name)(),
        })
    }))
    .map_err(|_| Error::CandidateFactoryPanicked {
        component: descriptor.id,
    })?
}
