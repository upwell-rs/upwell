//! Scope instances opened for an entry, deferred while the entry's plan builds nothing.

use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use tokio::sync::OnceCell;
use upwell_core::Scope;
use upwell_di::{BoxedComponent, Component, EntryPlan, FromContainer, ScopeContainer};

use super::{Builds, open_in};
use crate::entry::EntryId;
use crate::runtime::RuntimeView;

/// An instance of scope `S` opened for an entry by [`ScopeBuilder::open_when_needed`].
///
/// When the entry's plan builds nothing, opening allocates no container. Until it opens, reads
/// that never build, such as [`get`](Self::get), see only the scopes the instance opens under:
/// the instance's own slots, its seeds included, are empty. The container opens the first time
/// something resolves from the instance, on the runtime generation the entry was planned on,
/// and only then receives the builder's seeds. Any other entry opens the container at once.
///
/// [`ScopeBuilder::open_when_needed`]: super::ScopeBuilder::open_when_needed
pub struct EntryScope<S: Scope> {
    state: State,
    scope: PhantomData<fn() -> S>,
}

/// Whether an [`EntryScope`]'s container opened with it or waits for its first resolution.
///
/// An open instance is its container and the plan of the entry it opened for, if any; the
/// deferred state, once-cell included, lives behind its own box so it costs nothing when the
/// container opens at once.
enum State {
    Open {
        container: Arc<ScopeContainer>,
        plan: Option<Arc<EntryPlan>>,
    },
    Deferred(Box<Deferred>),
}

/// What a deferred [`EntryScope`] opens from, and its container once open.
struct Deferred {
    view: RuntimeView,
    parent: Arc<ScopeContainer>,
    entry: EntryId,
    seeds: Mutex<Option<Vec<BoxedComponent>>>,
    container: OnceCell<Arc<ScopeContainer>>,
}

impl<S: Scope> EntryScope<S> {
    /// An instance whose container is already open.
    pub fn from_container(container: Arc<ScopeContainer>) -> Self {
        Self::opened(container, None)
    }

    /// An instance whose container opened at once for an entry planned as `plan`.
    pub(super) fn opened(container: Arc<ScopeContainer>, plan: Option<Arc<EntryPlan>>) -> Self {
        Self {
            state: State::Open { container, plan },
            scope: PhantomData,
        }
    }

    pub(super) fn deferred(
        view: RuntimeView,
        parent: Arc<ScopeContainer>,
        entry: EntryId,
        seeds: Vec<BoxedComponent>,
    ) -> Self {
        Self {
            state: State::Deferred(Box::new(Deferred {
                view,
                parent,
                entry,
                seeds: Mutex::new(Some(seeds)),
                container: OnceCell::new(),
            })),
            scope: PhantomData,
        }
    }

    /// Whether the instance's container is open.
    pub fn is_open(&self) -> bool {
        match &self.state {
            State::Open { .. } => true,
            State::Deferred(deferred) => deferred.container.initialized(),
        }
    }

    /// The instance's container once it is open, or the scope it opens under until then.
    ///
    /// Before the container opens, reads through it see only the scopes the instance opens
    /// under, not the builder's seeds.
    pub fn current(&self) -> &Arc<ScopeContainer> {
        match &self.state {
            State::Open { container, .. } => container,
            State::Deferred(deferred) => deferred.container.get().unwrap_or(&deferred.parent),
        }
    }

    /// The component of type `T` as its handle, if it is built in the instance or the scopes
    /// it opens under. Never builds or opens anything, so before the container opens it does
    /// not see the builder's seeds.
    pub fn get<T: Component>(&self) -> Option<T::Handle> {
        self.current().get::<T>()
    }

    /// The instance's container, opening it first if it is deferred.
    ///
    /// # Errors
    ///
    /// Fails when the deferred container fails to open, now or on an earlier call.
    pub async fn container(&self) -> crate::Result<&Arc<ScopeContainer>> {
        match &self.state {
            State::Open { container, .. } => Ok(container),
            State::Deferred(deferred) => deferred.open::<S>().await,
        }
    }

    /// Resolves `H` from the instance, opening its container first if it is deferred.
    ///
    /// # Errors
    ///
    /// Fails when the container fails to open or `H` fails to resolve.
    pub async fn extract<H: FromContainer>(&self) -> crate::Result<H> {
        if let Some(value) = self.read_planned::<H>() {
            return Ok(value);
        }

        Box::pin(self.extract_opening::<H>()).await
    }

    /// Extracts `H`, opening the container first if it is deferred.
    ///
    /// Boxed by [`extract`](Self::extract) because opening a scope and selecting providers make
    /// a large future: inline, it would grow every handler future that extracts anything,
    /// though a planned target never awaits it.
    async fn extract_opening<H: FromContainer>(&self) -> crate::Result<H> {
        let container = self.container().await?;

        container
            .extract_planned::<H>(self.plan())
            .await
            .map_err(crate::Error::from)
    }

    /// The plan of the entry the instance opened for, when it opened for one.
    pub fn plan(&self) -> Option<&EntryPlan> {
        match &self.state {
            State::Open { plan, .. } => plan.as_deref(),
            State::Deferred(deferred) => deferred.view.scope_plan().layouts.entry(deferred.entry),
        }
    }

    /// `H` read from the source the entry's plan pre-resolved for it, when it is one of the
    /// entry's targets and is built. Opens nothing: before the container opens, only targets
    /// outside the instance's own scope are read.
    pub fn read_planned<H: FromContainer>(&self) -> Option<H> {
        H::read_planned(self.current(), self.plan()?)
    }
}

impl Deferred {
    /// The container, opened on the first call with the seeds and on the generation it was
    /// planned on.
    async fn open<S: Scope>(&self) -> crate::Result<&Arc<ScopeContainer>> {
        self.container
            .get_or_try_init(|| async {
                let seeds = self
                    .seeds
                    .lock()
                    .expect("deferred seeds lock poisoned")
                    .take()
                    .ok_or(crate::Error::DeferredScopeFailed { scope: S::ID })?;

                open_in(
                    self.view.clone(),
                    S::ID,
                    Arc::clone(&self.parent),
                    Builds::Entry(self.entry),
                    seeds,
                )
                .await
            })
            .await
    }
}
