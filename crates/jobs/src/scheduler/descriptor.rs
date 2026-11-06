//! The scheduler's hand-rolled DI descriptors.
//!
//! The `#[service]`/`#[hook]` macros cannot be used below the facade, so this is the runtime
//! metadata a `#[service] #[methods]` pair would otherwise emit.

use std::any::{Any, TypeId};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use upwell_core::{DependencyDescriptor, ResolverCtx, ResolverCtxExt, Scope, TypeDescriptor};
use upwell_di::{
    BoxedComponent, Component, ComponentConstructionContext, ComponentDescriptor,
    ComponentFactoryDescriptor, ComponentFactoryRole, ComponentSource, GenerationSnapshot,
    Result as DiResult, ScopedConstruction, Singleton, dispatch_factory, factory_dependencies,
};
use upwell_hooks::{Error as HookError, HookDescriptor, HookKind, Result as HookResult, Startup};

use super::{JobScheduler, SCHEDULER_ID, SCHEDULER_NAME};
impl Component for JobScheduler {
    type Handle = Arc<JobScheduler>;

    const ID: &'static str = SCHEDULER_ID;
    const NAME: &'static str = SCHEDULER_NAME;

    fn into_handle(self) -> Arc<JobScheduler> {
        Arc::new(self)
    }
}

/// Under `di-check`, the scheduler is plugin-seeded, so it is always provided — letting a user
/// component inject `Arc<JobScheduler>` to schedule jobs at run time.
#[cfg(feature = "di-check")]
impl upwell_di::Provided<JobScheduler> for upwell_di::Wiring {}

/// The scheduler's construction factory: resolves the injected [`RootResolver`] and builds it.
fn scheduler_construct<'a>(
    cx: &'a mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = DiResult<BoxedComponent>> + Send + 'a>> {
    dispatch_factory(JobScheduler::create, cx)
}

/// The scheduler's dependency edges, recovered from its factory's parameters.
fn scheduler_deps() -> Vec<DependencyDescriptor> {
    factory_dependencies(JobScheduler::create)
}

static SCHEDULER_FACTORIES: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: scheduler_construct,
    construct_ready: None,
    dependencies: scheduler_deps,
    role: ComponentFactoryRole::Explicit,
    scope: None,
}];

fn scheduler_factories() -> &'static [ComponentFactoryDescriptor] {
    &SCHEDULER_FACTORIES
}

/// The boxed future a hook call returns — the same shape `upwell-hooks` uses internally,
/// named here so the hand-rolled hook signature stays readable.
type HookCallFuture<'a> =
    Pin<Box<dyn Future<Output = HookResult<Box<dyn Any + Send>>> + Send + 'a>>;

/// The erased `Startup` hook: resolves the scheduler receiver through the component source and
/// starts its static jobs. Mirrors the body the `#[hook(Startup)]` macro would generate.
fn scheduler_startup_call<'a>(
    ctx: &'a (dyn ResolverCtx + Send + Sync),
    _cx: &'a (dyn Any + Send + Sync),
) -> HookCallFuture<'a> {
    Box::pin(async move {
        let scheduler = ctx
            .get_resolver::<ComponentSource>()
            .and_then(|source| source.component::<JobScheduler>())
            .ok_or(HookError::MissingReceiver(SCHEDULER_NAME))?;

        scheduler
            .start()
            .await
            .map_err(|error| HookError::Other(Box::new(error)))?;

        Ok(Box::new(()) as Box<dyn Any + Send>)
    })
}

fn scheduler_startup_kind_ty() -> TypeId {
    TypeId::of::<Startup>()
}

fn scheduler_startup_deps() -> Vec<DependencyDescriptor> {
    Vec::new()
}

static SCHEDULER_HOOKS: [HookDescriptor; 1] = [HookDescriptor::new(
    0,
    TypeDescriptor::of::<JobScheduler>(SCHEDULER_NAME),
    <Startup as HookKind>::NAME,
    scheduler_startup_kind_ty,
    scheduler_startup_deps,
    scheduler_startup_call,
)];

fn scheduler_hooks() -> &'static [HookDescriptor] {
    &SCHEDULER_HOOKS
}

/// The [`ComponentDescriptor`] for the scheduler singleton, registered by the
/// [`JobsPlugin`](crate::plugin::JobsPlugin).
impl upwell_core::Descriptor<ComponentDescriptor> for JobScheduler {
    const DESCRIPTOR: ComponentDescriptor = ComponentDescriptor {
        id: SCHEDULER_ID,
        name: SCHEDULER_NAME,
        ty: TypeDescriptor::of::<JobScheduler>(SCHEDULER_NAME),
        scope: Singleton::KEY,
        condition: None,
        factories: scheduler_factories,
        hooks: scheduler_hooks,
        construction: ScopedConstruction::OnDemand,
        generation_snapshot: Some(GenerationSnapshot::of::<JobScheduler>()),
        declared_scope: None,
    };
}

#[cfg(test)]
pub(crate) fn scheduler_descriptor() -> ComponentDescriptor {
    <JobScheduler as upwell_core::Descriptor<ComponentDescriptor>>::DESCRIPTOR
}
