//! Reading a trait's provider through the source its scope's layout selected, checked against
//! runtime provider selection over the chain the scope actually opened in.

#![allow(dead_code)]

use super::*;
use crate::Dep;
use crate::layout::EntryPlan;
use crate::test_support::{component, test_component};
use crate::{SlotLayout, dispatch_factory, factory_dependencies};

mod cases;
mod collections;

/// The trait every fixture provider provides.
trait Greeter: Send + Sync {
    fn name(&self) -> &'static str;
}

/// A scope opened under the root.
struct Outer;

/// A scope opened under the root or under [`Outer`].
struct Inner;

impl Scope for Outer {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/selected-outer");
    const RANK: u8 = 2;
    const NAME: &'static str = "Outer";
}

impl Scope for Inner {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/selected-inner");
    const RANK: u8 = 1;
    const NAME: &'static str = "Inner";
}

/// Declares a [`Greeter`] component `$ty` named `$id` in `$scope`, with a descriptor whose
/// factory builds it.
macro_rules! greeter {
    ($ty:ident, $id:literal, $scope:expr) => {
        #[doc = concat!("The `", $id, "` [`Greeter`].")]
        struct $ty;

        impl Greeter for $ty {
            fn name(&self) -> &'static str {
                $id
            }
        }

        test_component!($ty, $id, provides dyn Greeter);

        impl $ty {
            fn descriptor() -> ComponentDescriptor {
                async fn build() -> $ty {
                    $ty
                }

                fn factories() -> &'static [ComponentFactoryDescriptor] {
                    const {
                        &[ComponentFactoryDescriptor {
                            id: $id,
                            construct: |cx| dispatch_factory(build, cx),
                            construct_ready: None,
                            dependencies: || factory_dependencies(build),
                            role: ComponentFactoryRole::Explicit,
                        }]
                    }
                }

                let mut descriptor = component::<$ty>($id, factories);

                descriptor.scope = $scope;

                descriptor
            }

            fn provider() -> ProviderDescriptor {
                ProviderDescriptor::of::<$ty, dyn Greeter>("dyn Greeter")
            }
        }
    };
}

greeter!(RootGreeter, "root", Singleton::KEY);
greeter!(OuterGreeter, "outer", Outer::KEY);
greeter!(InnerPrimary, "inner-primary", Inner::KEY);
greeter!(InnerSpare, "inner-spare", Inner::KEY);
greeter!(FreshGreeter, "fresh", Transient::KEY);
greeter!(OuterAnchor, "outer-anchor", Outer::KEY);
greeter!(InnerAnchor, "inner-anchor", Inner::KEY);

/// The scopes [`Inner`]'s layout reaches when the generation is prepared.
#[derive(Clone, Copy)]
enum Reach {
    /// The root and itself: [`Inner`] also opens directly under the root, so [`Outer`] is
    /// not on every chain it opens in.
    EveryChain,
    /// [`Outer`] as well, as for a scope that always opens under it.
    Outer,
}

/// One prepared fixture generation: its registry, the root holding its singletons, and the
/// layouts of [`Outer`] and [`Inner`].
struct Generation {
    registry: Arc<ScopeRegistry>,
    root: Arc<ScopeContainer>,
    outer: Arc<SlotLayout>,
    inner: Arc<SlotLayout>,
}

impl Generation {
    /// Prepares `components` with `providers` in `ordinals` order, planning [`Inner`] with
    /// `reach`.
    async fn new(
        components: &[ComponentDescriptor],
        providers: Vec<ProviderDescriptor>,
        ordinals: HashMap<TypeId, HashMap<TypeId, usize>>,
        reach: Reach,
    ) -> Self {
        let in_scope = |scope: ScopeKey| -> Vec<ComponentDescriptor> {
            components
                .iter()
                .filter(|component| component.scope == scope)
                .copied()
                .collect()
        };
        let can_reach = |consumer: ScopeId, dependency: ScopeId| {
            consumer == dependency
                || dependency == Singleton::ID
                || matches!(reach, Reach::Outer) && dependency == Outer::ID
        };
        let selection = crate::registry::ProviderSelectionModel::new(
            components,
            providers.clone(),
            ordinals.clone(),
        )
        .expect("the providers name registered components");
        let outer = Arc::new(
            SlotLayout::plan(
                Outer::KEY,
                &[],
                &in_scope(Outer::KEY),
                &HashMap::new(),
                &selection,
                can_reach,
            )
            .expect("the outer scope plans"),
        );
        let inner = Arc::new(
            SlotLayout::plan(
                Inner::KEY,
                &[],
                &in_scope(Inner::KEY),
                &HashMap::from([(Outer::ID, Arc::clone(&outer))]),
                &selection,
                can_reach,
            )
            .expect("the inner scope plans"),
        );
        let (transient, stored): (HashMap<_, _>, HashMap<_, _>) = components
            .iter()
            .map(|component| (component.ty.type_id, *component))
            .partition(|(_, component)| component.scope.is_transient());
        let registry = Arc::new(
            ScopeRegistry::new(transient, stored, providers, ordinals)
                .expect("the scope registry validates"),
        );
        let root = ScopeContainer::build_root(
            &in_scope(Singleton::KEY),
            Vec::new(),
            ResolverSet::new(),
            Arc::clone(&registry),
        )
        .await
        .expect("the root builds");

        Self {
            registry,
            root,
            outer,
            inner,
        }
    }

    /// Prepares `components` with `providers` in declaration order.
    async fn of(
        components: &[ComponentDescriptor],
        providers: Vec<ProviderDescriptor>,
        reach: Reach,
    ) -> Self {
        Self::new(components, providers, HashMap::new(), reach).await
    }

    /// Opens [`Outer`] under the root from its layout, building every slot.
    async fn open_outer(&self) -> Arc<ScopeContainer> {
        self.open(Arc::clone(&self.root), &self.outer, None).await
    }

    /// Opens [`Inner`] under `parent` from its layout, building what `plan` builds, or every
    /// slot without one.
    async fn open_inner(
        &self,
        parent: &Arc<ScopeContainer>,
        plan: Option<&EntryPlan>,
    ) -> Arc<ScopeContainer> {
        self.open(Arc::clone(parent), &self.inner, plan).await
    }

    async fn open(
        &self,
        parent: Arc<ScopeContainer>,
        layout: &Arc<SlotLayout>,
        plan: Option<&EntryPlan>,
    ) -> Arc<ScopeContainer> {
        ScopeContainer::open_planned(
            parent,
            Arc::clone(&self.registry),
            layout,
            plan,
            Vec::new(),
            Arc::new(()),
        )
        .await
        .expect("the scope opens")
    }
}

/// The built provider runtime selection picks for [`Greeter`] with `qualifier` from `scope`.
fn selected_at_runtime(
    scope: &ScopeContainer,
    qualifier: Option<&str>,
) -> Option<Arc<dyn Greeter>> {
    let provider = scope.registry.selected_runtime_provider(
        TypeId::of::<dyn Greeter>(),
        qualifier,
        ResolutionMode::Eager,
        scope.scope,
        &|dependency| scope.can_access(dependency),
    )?;

    scope.resolve_provider_built::<Arc<dyn Greeter>>(&provider)
}

/// The name of the [`Greeter`] `scope` reads through its layout's selected source, asserting
/// that whatever it reads is the instance runtime selection resolves. `None` when the read
/// falls back to runtime selection.
fn read_selected(scope: &ScopeContainer, qualifier: Option<&str>) -> Option<&'static str> {
    let read = scope.selected_built::<Arc<dyn Greeter>>(TypeId::of::<dyn Greeter>(), qualifier)?;
    let selected = selected_at_runtime(scope, qualifier)
        .expect("runtime selection resolves a built provider where the table read one");

    assert!(
        std::ptr::addr_eq(Arc::as_ptr(&read), Arc::as_ptr(&selected)),
        "the table read `{}` but runtime selection resolves `{}`",
        read.name(),
        selected.name()
    );

    Some(read.name())
}

/// The name of the [`Greeter`] extracting `Arc<dyn Greeter>` from `scope` resolves.
async fn extracted(scope: &Arc<ScopeContainer>) -> &'static str {
    scope
        .extract::<Arc<dyn Greeter>>()
        .await
        .expect("the greeter extracts")
        .name()
}
