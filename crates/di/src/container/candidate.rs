use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use futures::FutureExt;
use tracing::{debug, info, instrument, trace};
use upwell_core::{ResolverSet, Singleton, StaticScope};

use super::{ComponentSource, ScopeContainer, ScopeRegistry, ScopeResolverSlot};
use crate::descriptors::component::ComponentConstructionContext;
use crate::{
    BoxedComponent, ComponentDescriptor, Error, Injectable, ROOT_RESOLVER_ID, RootResolver,
};

impl ScopeContainer {
    /// Constructs an isolated candidate singleton root.
    ///
    /// Retained instances must be reboxed into generation-local storage by the caller. Candidate
    /// factories resolve only from these retained instances, components built earlier in `order`,
    /// and the explicitly supplied external resolvers. An active root is never used as a parent.
    #[doc(hidden)]
    #[instrument(skip_all, fields(count = order.len()))]
    pub async fn build_candidate_root(
        order: &[&'static str],
        retained: Vec<BoxedComponent>,
        externals: ResolverSet,
        registry: Arc<ScopeRegistry>,
    ) -> crate::Result<Arc<ScopeContainer>> {
        if externals.get_arc::<ComponentSource>().is_some() {
            return Err(Error::CandidateActiveResolver);
        }

        let order = catch_unwind(AssertUnwindSafe(|| {
            validate_candidate_inputs(order, &retained, &registry)
        }))
        .map_err(|_| Error::CandidateMetadataPanicked)??;

        let slot = ScopeResolverSlot::default();
        let mut context = ComponentConstructionContext::new_with_slot(
            &Singleton,
            None,
            Arc::clone(&registry),
            externals,
            slot.clone(),
        );

        if let Some(descriptor) = registry.component_by_id(ROOT_RESOLVER_ID) {
            context.insert(BoxedComponent {
                ty: descriptor.ty,
                value: Box::new(Injectable::into_stored(RootResolver::new())),
            });
            register_candidate_providers(&mut context, &registry, descriptor)?;
        }

        for component in retained {
            let type_id = component.ty.type_id;
            let descriptor = registry
                .component(type_id)
                .expect("candidate inputs were validated");

            context.insert(component);
            register_candidate_providers(&mut context, &registry, descriptor)?;
        }

        for descriptor in order {
            debug!(component = %descriptor.name, "constructing candidate component");

            let component = AssertUnwindSafe(async {
                let factory = descriptor
                    .effective_factory()?
                    .ok_or(Error::MissingComponent(descriptor.name))?;

                (factory.construct)(&mut context).await
            })
            .catch_unwind()
            .await
            .map_err(|_| Error::CandidateFactoryPanicked {
                component: descriptor.id,
            })??;

            validate_factory_output(descriptor, &component)?;

            context.insert(component);
            register_candidate_providers(&mut context, &registry, descriptor)?;

            trace!(component = %descriptor.name, "candidate component ready");
        }

        let root = freeze_candidate(context, slot)?;

        if let Some(resolver) = root.get::<RootResolver>() {
            resolver.attach(&root);
        }

        info!(
            count = root.store.components.len(),
            "candidate root prepared"
        );

        Ok(root)
    }
}

fn validate_candidate_inputs(
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

fn validate_factory_output(
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

fn register_candidate_providers(
    context: &mut ComponentConstructionContext,
    registry: &ScopeRegistry,
    descriptor: ComponentDescriptor,
) -> crate::Result<()> {
    catch_unwind(AssertUnwindSafe(|| {
        for provider in registry.providers_for(descriptor.ty.type_id) {
            let ordinal = registry.provider_ordinal(provider);
            context.register_provider(provider, ordinal);
        }
    }))
    .map_err(|_| Error::CandidateProviderPanicked {
        component: descriptor.id,
    })
}

fn freeze_candidate(
    context: ComponentConstructionContext,
    slot: ScopeResolverSlot,
) -> crate::Result<Arc<ScopeContainer>> {
    catch_unwind(AssertUnwindSafe(|| {
        let parts = context.into_parts();

        let container = Arc::new_cyclic(|weak| {
            let mut resolvers = parts.resolvers;
            resolvers.insert(Arc::new(ComponentSource {
                container: weak.clone(),
            }));

            ScopeContainer {
                scope: parts.scope,
                store: parts.store,
                parent: parts.parent,
                registry: parts.registry,
                resolver_base: resolvers.clone(),
                resolvers: std::sync::OnceLock::from(resolvers),
                slot: slot.clone(),
                generation_lease: None,
            }
        });

        slot.attach(&container)?;

        Ok(container)
    }))
    .map_err(|_| Error::CandidateFinalizationPanicked)?
}

#[cfg(test)]
mod tests;
