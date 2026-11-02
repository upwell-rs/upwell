//! The inputs of one scope instance and how it opens.

use std::any::{Any, TypeId};
use std::marker::PhantomData;
use std::sync::Arc;

use upwell_core::{Scope, ScopeId, Singleton, TypeDescriptor};
use upwell_di::{BoxedComponent, EntryPlan, ScopeContainer};

use super::{EntryScope, OpenScope, OpensUnder, ScopeEntry, ScopeOpener};
use crate::entry::EntryId;
use crate::runtime::RuntimeView;
use crate::scope::ScopeParent;

/// The slots opening an instance builds at once; the rest are built on demand.
#[derive(Clone, Copy, Debug)]
pub(super) enum Builds {
    /// What the entry's plan lists, or every slot when the generation has no plan for it.
    Entry(EntryId),
    /// Every slot of the scope.
    Everything,
    /// Only the seeds and the eager set.
    Eager,
}

/// Which slots opening an instance builds at once, for
/// [`open_with_bound_seed`](ScopeBuilder::open_with_bound_seed).
#[non_exhaustive]
pub enum Opening<'e, S: Scope> {
    /// What the entry's plan lists, as [`open`](ScopeBuilder::open) does.
    Entry(&'e ScopeEntry<S>),
    /// Every slot of the scope, as [`open_default`](ScopeBuilder::open_default) does.
    Everything,
    /// Only the seeds and the eager set, as [`open_on_demand`](ScopeBuilder::open_on_demand)
    /// does.
    OnDemand,
}

/// The inputs of one instance of scope `S` opened under a parent of scope `P`.
///
/// A stack value: it allocates nothing until a seed is added.
#[must_use = "a scope builder does nothing until it is opened"]
pub struct ScopeBuilder<'a, S: Scope, P: Scope = Singleton> {
    opener: &'a ScopeOpener<S>,
    parent: Option<&'a OpenScope<P>>,
    seeds: Vec<BoxedComponent>,
}

impl<'a, S: Scope> ScopeBuilder<'a, S> {
    pub(super) fn new(opener: &'a ScopeOpener<S>) -> Self {
        Self {
            opener,
            parent: None,
            seeds: Vec::new(),
        }
    }
}

impl<'a, S: Scope, P: Scope> ScopeBuilder<'a, S, P> {
    /// Opens the instance under `parent`, on the runtime generation `parent` runs on.
    pub fn under<Q: Scope>(self, parent: &'a OpenScope<Q>) -> ScopeBuilder<'a, S, Q>
    where
        S: OpensUnder<Q>,
    {
        ScopeBuilder {
            opener: self.opener,
            parent: Some(parent),
            seeds: self.seeds,
        }
    }

    /// Fills the scope's factory-less slot for `T` with `value`.
    pub fn seed<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.seeds.push(boxed_seed(value));

        self
    }
}

impl<S: OpensUnder<P>, P: Scope> ScopeBuilder<'_, S, P> {
    /// Opens the instance to run `entry`: it builds what the entry's plan lists, the entry's
    /// targets and what they depend on in the scope, and builds any other slot on demand when
    /// something resolves it.
    ///
    /// # Errors
    ///
    /// Fails when the parent belongs to another runtime, a seed has no slot in the scope or
    /// shares one with another seed, or a component fails to build.
    pub async fn open(self, entry: &ScopeEntry<S>) -> crate::Result<OpenScope<S>> {
        self.open_with(Builds::Entry(entry.id)).await
    }

    /// Opens the instance without an entry: it builds every slot of the scope.
    ///
    /// # Errors
    ///
    /// As [`open`](Self::open).
    pub async fn open_default(self) -> crate::Result<OpenScope<S>> {
        self.open_with(Builds::Everything).await
    }

    /// Opens the instance building only its seeds and its eager components: every other slot
    /// is built when something resolves it, so the instance costs only what it is used for.
    ///
    /// # Errors
    ///
    /// Fails when the parent belongs to another runtime, or a seed has no slot in the scope or
    /// shares one with another seed.
    pub async fn open_on_demand(self) -> crate::Result<OpenScope<S>> {
        self.open_with(Builds::Eager).await
    }

    /// Opens the instance to run `entry` as [`open`](Self::open) does, or without an entry as
    /// [`open_default`](Self::open_default) does, unless the entry's plan builds nothing (an
    /// eager component of the scope makes every plan build it): then the container opens the
    /// first time something resolves from the returned scope, with this builder's seeds.
    /// Until then, reads that never build see only the scopes it opens under, not the seeds.
    ///
    /// # Errors
    ///
    /// As [`open`](Self::open). Seeds and components of an entry that plans nothing are
    /// checked when the container opens.
    pub async fn open_when_needed(
        self,
        entry: Option<&ScopeEntry<S>>,
    ) -> crate::Result<EntryScope<S>> {
        let (view, parent) = self.target()?;
        let builds = match entry {
            Some(entry) if plans_nothing(&view, entry) => {
                return Ok(EntryScope::deferred(view, parent, entry.id, self.seeds));
            }

            Some(entry) => Builds::Entry(entry.id),
            None => Builds::Everything,
        };
        let plan =
            entry.and_then(|entry| view.scope_plan().layouts.shared_entry(entry.id).cloned());
        let container = open_in(view, S::ID, parent, builds, self.seeds).await?;

        Ok(EntryScope::opened(container, plan))
    }

    /// Opens the instance as `opening` says, adding the seed of type `T` from `source` only when
    /// that opening reads it, on the generation the instance opens on. A seed its producer
    /// would pay for, such as a copy of a request head, then costs nothing when nothing reads
    /// it; [`ScopeContainer::ensure_seed`] fills it later for a read the opening could not see.
    ///
    /// # Errors
    ///
    /// As the method `opening` names.
    pub async fn open_with_bound_seed<T: Any + Send + Sync>(
        mut self,
        opening: Opening<'_, S>,
        source: impl FnOnce() -> T,
    ) -> crate::Result<OpenScope<S>> {
        let (view, parent) = self.target()?;
        let builds = match opening {
            Opening::Entry(entry) => Builds::Entry(entry.id),
            Opening::Everything => Builds::Everything,
            Opening::OnDemand => Builds::Eager,
        };

        if reads_seed(&view, S::ID, builds, TypeId::of::<T>()) {
            self.seeds.push(boxed_seed(source()));
        }

        let container = open_in(view, S::ID, parent, builds, self.seeds).await?;

        Ok(OpenScope {
            container,
            scope: PhantomData,
        })
    }

    async fn open_with(self, builds: Builds) -> crate::Result<OpenScope<S>> {
        let (view, parent) = self.target()?;
        let container = open_in(view, S::ID, parent, builds, self.seeds).await?;

        Ok(OpenScope {
            container,
            scope: PhantomData,
        })
    }

    /// The generation the instance opens on and the container it opens under.
    fn target(&self) -> crate::Result<(RuntimeView, Arc<ScopeContainer>)> {
        let runtime = &self.opener.runtime;
        let (view, parent) = match self.parent {
            Some(parent) => (
                runtime.pinned_view(S::ID, &parent.container)?,
                Arc::clone(&parent.container),
            ),
            None => {
                let view = runtime.view();
                let root = Arc::clone(view.root());

                (view, root)
            }
        };

        debug_assert!(
            opens_under::<S, P>(&view),
            "`OpensUnder` lists a parent the topology of `{}` does not",
            S::NAME
        );

        Ok((view, parent))
    }
}

/// Whether the plan `view` prepared for `entry` builds nothing.
fn plans_nothing<S: Scope>(view: &RuntimeView, entry: &ScopeEntry<S>) -> bool {
    view.scope_plan()
        .layouts
        .entry(entry.id)
        .is_some_and(EntryPlan::is_empty)
}

fn boxed_seed<T: Any + Send + Sync>(value: T) -> BoxedComponent {
    BoxedComponent::stored(TypeDescriptor::of::<T>(std::any::type_name::<T>()), value)
}

/// Whether opening an instance of `scope` on `view` with `builds` reads the seed of type `seed`.
fn reads_seed(view: &RuntimeView, scope: ScopeId, builds: Builds, seed: TypeId) -> bool {
    let layouts = &view.scope_plan().layouts;
    let Some(layout) = layouts.get(&scope) else {
        return false;
    };

    match builds {
        Builds::Entry(entry) => match layouts.entry(entry) {
            Some(plan) => plan.reads_seed(layout, seed),
            None => layout.factories_read_seed(seed),
        },
        Builds::Everything => layout.factories_read_seed(seed),
        Builds::Eager => layout.eager_set_reads_seed(seed),
    }
}

/// Builds an instance of `scope` under `parent` from the layout `view` prepared for it, with
/// the slots `builds` names.
pub(super) async fn open_in(
    view: RuntimeView,
    scope: ScopeId,
    parent: Arc<ScopeContainer>,
    builds: Builds,
    seeds: Vec<BoxedComponent>,
) -> crate::Result<Arc<ScopeContainer>> {
    let layouts = &view.scope_plan().layouts;
    let layout = layouts
        .get(&scope)
        .ok_or(crate::Error::UndeclaredScopeOpen { scope })?;
    let on_demand;
    let plan = match builds {
        Builds::Entry(entry) => layouts.entry(entry),
        Builds::Everything => None,
        Builds::Eager => {
            on_demand = EntryPlan::on_demand(layout);

            Some(&on_demand)
        }
    };

    ScopeContainer::open_planned(
        parent,
        Arc::clone(view.scopes()),
        layout,
        plan,
        seeds,
        view.generation_state(),
    )
    .await
    .map_err(crate::Error::from)
}

/// Whether the topology `view` runs on lets `S` open under `P`.
fn opens_under<S: Scope, P: Scope>(view: &RuntimeView) -> bool {
    let parent = if P::ID == Singleton::ID {
        ScopeParent::Root
    } else {
        ScopeParent::of::<P>()
    };

    view.scope_plan()
        .topology
        .boundary(&S::ID)
        .is_some_and(|boundary| boundary.opens_under(parent))
}
