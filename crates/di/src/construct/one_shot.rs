//! Transition-local one-shot factories.
//!
//! A component may declare a transition handler: a plain function that receives the
//! planned change for that component and either returns a [`OneShot`] factory for that
//! one transition or declines. A declined change, and every later transition, falls back
//! to the component's declared strategy. A one-shot factory changes *how* the component
//! is constructed, never *what* it depends on: every dependency it resolves must be
//! declared by the component's ordinary factory, so the transition plan has scheduled it.

use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use upwell_core::DependencyDescriptor;

use super::handoff::Handoff;
use super::{Factory, ReplacementFactory};
use crate::descriptors::{BoxedComponent, Component, ComponentConstructionContext};
use crate::{NodeAction, PlannedNode, ReasonKind, TransitionReason};

type ConstructFuture<'a> = Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + 'a>>;

type ErasedConstruct =
    Box<dyn for<'a> FnOnce(&'a mut ComponentConstructionContext) -> ConstructFuture<'a> + Send>;

/// The planned change a component's transition handler decides on.
///
/// It carries the component's structural action and the categorized reasons with their
/// dependency paths, never current or candidate config values.
#[derive(Clone, Debug)]
pub struct ComponentChange {
    node: PlannedNode,
}

/// A transition-local factory for component `C`, constructed at most once.
///
/// Returned by a transition handler for one planned change. It runs during candidate
/// construction for that attempt only and is dropped afterwards, whatever the outcome.
/// It may capture change-specific values; [`replacing`](Self::replacing) also reads the
/// previous instance through [`Previous`](super::Previous).
pub struct OneShot<C: Component> {
    erased: ErasedOneShot,
    _component: PhantomData<fn() -> C>,
}

/// A type-erased [`OneShot`], with the dependency edges it will resolve.
pub(crate) struct ErasedOneShot {
    pub(crate) dependencies: Vec<DependencyDescriptor>,
    pub(crate) construct: ErasedConstruct,
}

impl ComponentChange {
    #[doc(hidden)]
    pub fn new(node: PlannedNode) -> Self {
        Self { node }
    }

    /// The stable ID of the changing component.
    pub fn component(&self) -> &'static str {
        self.node.component
    }

    /// The minimum structural action the transition requires.
    pub fn action(&self) -> NodeAction {
        self.node.action
    }

    /// Every reason requiring the action, with the dependency path that caused it.
    pub fn reasons(&self) -> &[TransitionReason] {
        &self.node.reasons
    }

    /// Whether any reason has category `kind`.
    pub fn has_reason(&self, kind: ReasonKind) -> bool {
        self.node.reasons.iter().any(|reason| reason.kind == kind)
    }
}

impl<C: Component> OneShot<C> {
    /// A one-shot factory built from an ordinary factory function or closure.
    pub fn new<F, Args>(factory: F) -> Self
    where
        F: Factory<Args> + Send + 'static,
        Args: 'static,
    {
        Self::erased(
            F::dependencies(),
            Box::new(move |cx| Box::pin(factory.construct(cx))),
        )
    }

    /// A one-shot factory whose first parameter is the previous instance.
    pub fn replacing<F, Args>(factory: F) -> Self
    where
        F: ReplacementFactory<Args> + Send + 'static,
        Args: 'static,
    {
        Self::erased(
            F::dependencies(),
            Box::new(move |cx| Box::pin(factory.construct(cx))),
        )
    }

    fn erased(dependencies: Vec<DependencyDescriptor>, construct: ErasedConstruct) -> Self {
        Self {
            erased: ErasedOneShot {
                dependencies,
                construct,
            },
            _component: PhantomData,
        }
    }
}

/// Runs a transition `handler` against the change set on `cx`, boxing its selection.
///
/// Erased to the [`ComponentFactory`](crate::descriptors::ComponentFactory) fn-pointer
/// shape; the candidate builder unboxes the selection with [`take_selection`].
pub fn dispatch_transition<'a, C, H>(
    handler: H,
    cx: &'a mut ComponentConstructionContext,
) -> ConstructFuture<'a>
where
    C: Component,
    H: FnOnce(&ComponentChange) -> Option<OneShot<C>> + Send + 'a,
{
    Box::pin(async move {
        let change = cx
            .change()
            .ok_or(crate::Error::ChangeUnavailable { component: C::ID })?;
        let selected = handler(change).map(|one_shot| one_shot.erased);

        Ok(Handoff::boxed(selected))
    })
}

/// Unboxes a transition handler's selection: the selected one-shot factory, or `None`
/// when it declined. The outer `None` means `boxed` is not a selection.
pub(crate) fn take_selection(boxed: BoxedComponent) -> Option<Option<ErasedOneShot>> {
    Handoff::take(boxed)
}
