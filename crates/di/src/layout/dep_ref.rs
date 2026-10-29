use std::any::TypeId;
use std::sync::Arc;

use upwell_core::Cardinality;

use super::{SlotIdx, SlotLayout};
use crate::descriptors::{ComponentFactoryDescriptor, ProviderDescriptor};

/// Where a built instance, or one of its provider aliases, sits in a scope's slots.
///
/// `alias` indexes the slot's provider list; `None` is the concrete instance itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SlotRef {
    pub(crate) slot: SlotIdx,
    pub(crate) alias: Option<usize>,
}

/// The root singleton a dependency resolves to. A provider descriptor is boxed so a
/// planned edge stays small.
#[derive(Clone, Debug)]
pub(crate) enum RootTarget {
    Component(TypeId),
    Provider(Box<ProviderDescriptor>),
}

/// The source a planned dependency edge resolves to, fixed when the generation is prepared.
#[derive(Clone, Debug)]
pub(crate) enum DepRef {
    /// A singleton held by the root container.
    Root(RootTarget),
    /// An instance built in an enclosing scope, at a slot of that scope's layout. The
    /// layout is kept so the slot is only read from a store opened from it.
    Ancestor {
        layout: Arc<SlotLayout>,
        at: SlotRef,
    },
    /// An instance built earlier in the same scope.
    Local(SlotRef),
    /// A transient built for every resolution.
    Transient(Arc<TransientPlan>),
    /// Every provider of a collection or keyed dependency, in provider order.
    Many(Box<[ProvidedRef]>),
    /// An optional dependency no visible provider satisfies.
    Absent,
}

/// One provider of a collection or keyed dependency.
#[derive(Clone, Debug)]
pub(crate) struct ProvidedRef {
    pub(crate) qualifier: &'static str,
    pub(crate) source: DepRef,
}

/// A transient's construction recipe with its own planned dependencies.
///
/// Validation lets a transient depend only on singletons and other transients, so a plan
/// never refers to the slots of the scope that requests it.
#[derive(Debug)]
pub(crate) struct TransientPlan {
    pub(crate) factory: &'static ComponentFactoryDescriptor,
    pub(crate) provider: Option<ProviderDescriptor>,
    pub(crate) dependencies: Box<[PlannedDependency]>,
}

/// One eager dependency edge of a factory and the source it resolves to.
#[derive(Clone, Debug)]
pub(crate) struct PlannedDependency {
    pub(crate) target: TypeId,
    pub(crate) qualifier: Option<&'static str>,
    pub(crate) cardinality: Cardinality,
    pub(crate) source: DepRef,
}

impl PlannedDependency {
    /// Returns the source of the edge requesting `target` with `qualifier` and `cardinality`.
    pub(crate) fn find<'plan>(
        planned: &'plan [Self],
        target: TypeId,
        qualifier: Option<&str>,
        cardinality: Cardinality,
    ) -> Option<&'plan DepRef> {
        let edge = planned.iter().find(|edge| {
            edge.target == target && edge.qualifier == qualifier && edge.cardinality == cardinality
        })?;

        Some(&edge.source)
    }
}
