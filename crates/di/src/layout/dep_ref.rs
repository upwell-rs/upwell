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
    /// An edge resolved through its own handle (config, dynamic, lazy, deferred or fresh),
    /// kept so planned edges stay at their declared positions.
    Unplanned,
}

impl DepRef {
    /// Whether resolving this source builds a transient, which reads the requesting scope's
    /// in-progress store.
    pub(crate) fn builds_transient(&self) -> bool {
        match self {
            Self::Transient(_) => true,
            Self::Many(provided) => provided.iter().any(|entry| entry.source.builds_transient()),
            Self::Root(_)
            | Self::Ancestor { .. }
            | Self::Local(_)
            | Self::Absent
            | Self::Unplanned => false,
        }
    }
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

/// One dependency edge of a factory, at its declared position, and the source it resolves
/// to.
#[derive(Clone, Debug)]
pub(crate) struct PlannedDependency {
    pub(crate) target: TypeId,
    pub(crate) qualifier: Option<&'static str>,
    pub(crate) cardinality: Cardinality,
    pub(crate) source: DepRef,
}

impl PlannedDependency {
    /// Returns the planned source of the edge requesting `target` with `qualifier` and
    /// `cardinality`.
    pub(crate) fn find<'plan>(
        planned: &'plan [Self],
        target: TypeId,
        qualifier: Option<&str>,
        cardinality: Cardinality,
    ) -> Option<&'plan DepRef> {
        let edge = planned.iter().find(|edge| {
            edge.target == target
                && edge.qualifier == qualifier
                && edge.cardinality == cardinality
                && edge.is_planned()
        })?;

        Some(&edge.source)
    }

    /// Returns the planned source of the edge declared at `index`, when it requests
    /// `target` with `cardinality`.
    ///
    /// A planned edge requested as another type means a factory read an edge away from
    /// its declared position, which is a bug in the factory's generated code. Debug builds
    /// panic on it; release builds return `None` so the caller falls back to a typed
    /// lookup instead of reading another edge's slot.
    pub(crate) fn at(
        planned: &[Self],
        index: usize,
        target: TypeId,
        cardinality: Cardinality,
    ) -> Option<&DepRef> {
        let edge = planned.get(index)?;

        if !edge.is_planned() {
            return None;
        }

        let declared = edge.target == target && edge.cardinality == cardinality;

        debug_assert!(
            declared,
            "factory edge {index} was requested as another type than it declared; this is a \
             bug in the factory's generated code"
        );

        declared.then_some(&edge.source)
    }

    fn is_planned(&self) -> bool {
        !matches!(self.source, DepRef::Unplanned)
    }
}
