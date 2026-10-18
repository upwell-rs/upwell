use std::{fmt, future::Future, pin::Pin};

use upwell_core::{DependencyDescriptor, UpwellDescriptor};
use upwell_hooks::HookDescriptor;

use super::{BoxedComponent, ComponentConstructionContext, ComponentDescriptor};

/// Async factory function pointer for constructing a component.
pub type ComponentFactory =
    for<'a> fn(
        &'a mut ComponentConstructionContext,
    ) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + 'a>>;

/// How the container uses one of a component's construction recipes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComponentFactoryRole {
    /// The field-injection default, used only when no explicit factory exists.
    Default,
    /// An `#[init]` or `factory = path` constructor, preferred over the default.
    Explicit,
    /// A static state-aware replacement, used only when a runtime transition replaces an
    /// active instance. It reads the previous instance through
    /// [`Previous`](crate::Previous) and may resolve only dependencies the effective
    /// ordinary factory also declares.
    Replacement,
}

/// One way to construct a component, carrying the dependencies *that constructor*
/// needs (zero or many).
///
/// A component owns a slice of these (its `{Type}Factories` distributed slice): the
/// `#[component]`/`#[service]` field-injection [`Default`](ComponentFactoryRole::Default)
/// plus any [`Explicit`](ComponentFactoryRole::Explicit) factories contributed by an
/// `#[init]` or `factory = path`, and at most one
/// [`Replacement`](ComponentFactoryRole::Replacement). The effective ordinary one is chosen
/// by [`ComponentDescriptor::effective_factory`].
#[derive(Clone, Copy)]
pub struct ComponentFactoryDescriptor {
    /// Stable identity of this construction recipe within its owning component.
    ///
    /// This identity is part of graph transition comparison. Change it whenever the recipe's
    /// construction semantics change. IDs must be non-empty and unique within a component.
    pub id: &'static str,
    pub construct: ComponentFactory,
    /// The factory's dependency edges, reported at runtime. Read only at build.
    pub dependencies: fn() -> Vec<DependencyDescriptor>,
    pub role: ComponentFactoryRole,
}

impl fmt::Debug for ComponentFactoryDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComponentFactoryDescriptor")
            .field("id", &self.id)
            .field("dependencies", &(self.dependencies)())
            .field("role", &self.role)
            .finish_non_exhaustive()
    }
}

impl UpwellDescriptor for ComponentFactoryDescriptor {}

/// The merged element type of a component's single `{Type}Registrations` `linkme` slice.
///
/// Folding factories and hooks into one element type lets the base `#[component]` declare **one**
/// distributed slice — hence one linker section — instead of two. The `inventory` backend does
/// not use this: there, factories and hooks stay in separate `DescriptorFor` buckets (extra
/// collections cost no sections), so this enum is a linkme-path-only concern.
#[derive(Clone, Copy)]
pub enum Registration {
    Factory(ComponentFactoryDescriptor),
    Hook(HookDescriptor),
}

impl Registration {
    /// Wraps a factory.
    pub const fn factory(descriptor: ComponentFactoryDescriptor) -> Self {
        Self::Factory(descriptor)
    }

    /// Wraps a hook.
    pub const fn hook(descriptor: HookDescriptor) -> Self {
        Self::Hook(descriptor)
    }

    /// The factory, if this entry is one.
    pub fn as_factory(&self) -> Option<&ComponentFactoryDescriptor> {
        match self {
            Self::Factory(factory) => Some(factory),

            Self::Hook(_) => None,
        }
    }

    /// The hook, if this entry is one.
    pub fn as_hook(&self) -> Option<&HookDescriptor> {
        match self {
            Self::Hook(hook) => Some(hook),

            Self::Factory(_) => None,
        }
    }
}

impl UpwellDescriptor for Registration {}

/// A component type's own construction factories.
///
/// Implemented for each `#[component]`/`#[service]` by the macro to return that
/// type's `{Type}Factories` distributed slice.
pub trait ComponentFactories {
    /// Every factory contributed to this component type.
    fn factories() -> &'static [ComponentFactoryDescriptor];
}

impl ComponentDescriptor {
    /// The factory the container should use: an explicit one if present (the default
    /// is its fallback), or `None` for a manually-provided instance (no ordinary
    /// factory). Errors if more than one explicit factory exists for the type.
    /// Replacement factories never take part in this choice.
    pub fn effective_factory(&self) -> crate::Result<Option<&'static ComponentFactoryDescriptor>> {
        let mut explicit = self.factories_with(ComponentFactoryRole::Explicit);
        let first = explicit.next();

        if first.is_some() && explicit.next().is_some() {
            return Err(crate::Error::AmbiguousFactory(self.name.to_string()));
        }

        match first {
            Some(factory) => Ok(Some(factory)),

            None => Ok(self.factories_with(ComponentFactoryRole::Default).next()),
        }
    }

    /// The static state-aware replacement factory, if the component declares one.
    /// Errors if it declares more than one.
    pub fn replacement_factory(
        &self,
    ) -> crate::Result<Option<&'static ComponentFactoryDescriptor>> {
        let mut replacements = self.factories_with(ComponentFactoryRole::Replacement);
        let first = replacements.next();

        if first.is_some() && replacements.next().is_some() {
            return Err(crate::Error::AmbiguousReplacementFactory(
                self.name.to_string(),
            ));
        }

        Ok(first)
    }

    fn factories_with(
        &self,
        role: ComponentFactoryRole,
    ) -> impl Iterator<Item = &'static ComponentFactoryDescriptor> {
        (self.factories)()
            .iter()
            .filter(move |factory| factory.role == role)
    }

    /// Validates factory identities and the replacement contract: a replacement needs
    /// an ordinary factory and may resolve only dependencies that factory declares, so
    /// the planned construction order stays authoritative for it.
    pub(crate) fn validate_factories(&self) -> crate::Result<()> {
        let mut ids = std::collections::HashSet::new();

        for factory in (self.factories)() {
            if factory.id.is_empty() {
                return Err(crate::Error::EmptyFactoryId(self.name.to_string()));
            }

            if !ids.insert(factory.id) {
                return Err(crate::Error::DuplicateFactoryId {
                    component: self.name.to_string(),
                    factory: factory.id.to_string(),
                });
            }
        }

        let Some(replacement) = self.replacement_factory()? else {
            return Ok(());
        };
        let Some(ordinary) = self.effective_factory()? else {
            return Err(crate::Error::ReplacementWithoutFactory(
                self.name.to_string(),
            ));
        };
        let declared = (ordinary.dependencies)();

        for dependency in (replacement.dependencies)() {
            if !declared.iter().any(|edge| same_edge(edge, &dependency)) {
                return Err(crate::Error::UndeclaredReplacementDependency {
                    component: self.name.to_string(),
                    dependency: dependency.name.to_string(),
                });
            }
        }

        Ok(())
    }
}

/// Whether two edges select the same target, so one stands in for the other in the
/// planned graph. Observation is ignored: a replacement may read a `Dep<T>` edge as a
/// fixed `Arc<T>` or the reverse.
fn same_edge(left: &DependencyDescriptor, right: &DependencyDescriptor) -> bool {
    left.ty.type_id == right.ty.type_id
        && left.qualifier == right.qualifier
        && left.cardinality == right.cardinality
        && left.config == right.config
}
