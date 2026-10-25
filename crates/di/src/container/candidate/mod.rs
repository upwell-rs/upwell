use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use futures::FutureExt;
use tracing::{debug, info, instrument, trace};
use upwell_core::{ResolverSet, Scope, Singleton};

use super::{ComponentSource, ScopeContainer, ScopeRegistry, ScopeResolverSlot};
use crate::construct::{ErasedOneShot, take_selection};
use crate::descriptors::component::ComponentConstructionContext;
use crate::{
    BoxedComponent, ComponentChange, ComponentDescriptor, ComponentFactoryRole, Error, Injectable,
    ROOT_RESOLVER_ID, RootResolver,
};

mod validate;

use validate::{
    validate_candidate_inputs, validate_change_inputs, validate_factory_output,
    validate_previous_inputs,
};

impl ScopeContainer {
    /// Constructs an isolated candidate singleton root.
    ///
    /// Retained instances must be reboxed into generation-local storage by the caller. Candidate
    /// factories resolve only from these retained instances, components built earlier in `order`,
    /// and the explicitly supplied external resolvers. An active root is never used as a parent.
    ///
    /// `previous` holds generation-local snapshots of active instances being replaced. A
    /// component in `order` with a previous instance is built by its replacement factory,
    /// which reads that snapshot through [`Previous`](crate::Previous); every other one
    /// uses its ordinary effective factory.
    ///
    /// `changes` selects the components whose transition handler decides how they are
    /// built: the handler receives its component's planned change and either returns a
    /// one-shot factory, which runs once after its dependencies are checked against the
    /// ordinary factory, or declines, so the component is built as described above.
    #[doc(hidden)]
    #[instrument(skip_all, fields(count = order.len()))]
    pub async fn build_candidate_root(
        order: &[&'static str],
        retained: Vec<BoxedComponent>,
        previous: Vec<BoxedComponent>,
        changes: Vec<ComponentChange>,
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
        let mut previous = catch_unwind(AssertUnwindSafe(|| {
            validate_previous_inputs(&order, previous)
        }))
        .map_err(|_| Error::CandidateMetadataPanicked)??;
        let mut changes =
            catch_unwind(AssertUnwindSafe(|| validate_change_inputs(&order, changes)))
                .map_err(|_| Error::CandidateMetadataPanicked)??;

        let slot = ScopeResolverSlot::default();
        let mut context = ComponentConstructionContext::new_with_slot(
            Singleton::KEY,
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

            let replaced = previous.remove(&descriptor.ty.type_id);
            let change = changes.remove(descriptor.id);
            let component = AssertUnwindSafe(construct_candidate(
                &mut context,
                descriptor,
                replaced,
                change,
            ))
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

/// Builds one candidate component: through the one-shot factory its transition handler
/// selects for `change`, else through the replacement factory when `previous` is present
/// and declared, else through the ordinary effective factory. The previous snapshot and
/// the change are visible only while the factory reading them runs.
async fn construct_candidate(
    context: &mut ComponentConstructionContext,
    descriptor: ComponentDescriptor,
    previous: Option<BoxedComponent>,
    change: Option<ComponentChange>,
) -> crate::Result<BoxedComponent> {
    let one_shot = match change {
        Some(change) => select_one_shot(context, descriptor, change).await?,

        None => None,
    };

    context.set_previous(previous);

    let component = match one_shot {
        Some(one_shot) => (one_shot.construct)(context).await,

        None => construct_declared(context, descriptor).await,
    };

    context.set_previous(None);

    component
}

/// Runs `descriptor`'s transition handler for `change` and checks the one-shot factory it
/// selects against the ordinary factory's declared dependencies. `None` when it declines.
async fn select_one_shot(
    context: &mut ComponentConstructionContext,
    descriptor: ComponentDescriptor,
    change: ComponentChange,
) -> crate::Result<Option<ErasedOneShot>> {
    let handler = descriptor
        .transition_handler()?
        .ok_or(Error::MissingComponent(descriptor.name))?;

    context.set_change(Some(change));

    let selection = (handler.construct)(context).await;

    context.set_change(None);

    let selected = take_selection(selection?).ok_or(Error::CandidateFactoryTypeMismatch {
        component: descriptor.id,
        expected: "transition selection",
        actual: "<component>",
    })?;
    let Some(one_shot) = selected else {
        return Ok(None);
    };
    let ordinary = descriptor
        .effective_factory()?
        .ok_or(Error::MissingComponent(descriptor.name))?;

    descriptor.require_declared(
        &(ordinary.dependencies)(),
        &one_shot.dependencies,
        ComponentFactoryRole::Transition,
    )?;

    Ok(Some(one_shot))
}

/// Builds `descriptor` through its replacement factory when a previous instance is set
/// and the replacement is declared, through its ordinary effective factory otherwise.
async fn construct_declared(
    context: &mut ComponentConstructionContext,
    descriptor: ComponentDescriptor,
) -> crate::Result<BoxedComponent> {
    let replacement = if context.has_previous() {
        descriptor.replacement_factory()?
    } else {
        None
    };
    let factory = match replacement {
        Some(replacement) => Some(replacement),

        None => descriptor.effective_factory()?,
    };
    let factory = factory.ok_or(Error::MissingComponent(descriptor.name))?;

    (factory.construct)(context).await
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
                drop_guard: std::sync::OnceLock::new(),
            }
        });

        slot.attach(&container)?;

        Ok(container)
    }))
    .map_err(|_| Error::CandidateFinalizationPanicked)?
}

#[cfg(test)]
mod tests;
