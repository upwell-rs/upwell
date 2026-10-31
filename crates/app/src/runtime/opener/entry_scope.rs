//! Scope instances opened for an entry, deferred while the entry's plan builds nothing.

use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use tokio::sync::OnceCell;
use upwell_core::Scope;
use upwell_di::{BoxedComponent, Component, FromContainer, ScopeContainer};

use super::{Builds, open_in};
use crate::entry::EntryId;
use crate::runtime::RuntimeView;

/// An instance of scope `S` opened for an entry by [`ScopeBuilder::open_when_needed`].
///
/// When the entry's plan builds nothing, opening allocates no container. Reads that never
/// build, such as [`get`](Self::get), see the scope the instance opens under, which holds
/// everything such a read could find. The container opens the first time something resolves
/// from the instance, on the runtime generation the entry was planned on, with the builder's
/// seeds. Any other entry opens the container at once.
///
/// [`ScopeBuilder::open_when_needed`]: super::ScopeBuilder::open_when_needed
pub struct EntryScope<S: Scope> {
    container: OnceCell<Arc<ScopeContainer>>,
    deferred: Option<Deferred>,
    scope: PhantomData<fn() -> S>,
}

/// What a deferred [`EntryScope`] opens from.
struct Deferred {
    view: RuntimeView,
    parent: Arc<ScopeContainer>,
    entry: EntryId,
    seeds: Mutex<Option<Vec<BoxedComponent>>>,
}

impl<S: Scope> EntryScope<S> {
    /// An instance whose container is already open.
    pub fn from_container(container: Arc<ScopeContainer>) -> Self {
        Self {
            container: OnceCell::new_with(Some(container)),
            deferred: None,
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
            container: OnceCell::new(),
            deferred: Some(Deferred {
                view,
                parent,
                entry,
                seeds: Mutex::new(Some(seeds)),
            }),
            scope: PhantomData,
        }
    }

    /// Whether the instance's container is open.
    pub fn is_open(&self) -> bool {
        self.container.initialized()
    }

    /// The instance's container once it is open, or the scope it opens under until then.
    ///
    /// Reads through it that never build see the same components either way.
    pub fn current(&self) -> &Arc<ScopeContainer> {
        match (self.container.get(), &self.deferred) {
            (Some(container), _) => container,
            (None, Some(deferred)) => &deferred.parent,
            (None, None) => unreachable!("an entry scope without a container is deferred"),
        }
    }

    /// The component of type `T` as its handle, if it is built in the instance or the scopes
    /// it opens under. Never builds or opens anything.
    pub fn get<T: Component>(&self) -> Option<T::Handle> {
        self.current().get::<T>()
    }

    /// The instance's container, opening it first if it is deferred.
    ///
    /// # Errors
    ///
    /// Fails when the deferred container fails to open, now or on an earlier call.
    pub async fn container(&self) -> crate::Result<&Arc<ScopeContainer>> {
        self.container
            .get_or_try_init(|| async {
                let deferred = self
                    .deferred
                    .as_ref()
                    .expect("an entry scope without a container is deferred");
                let seeds = deferred
                    .seeds
                    .lock()
                    .expect("deferred seeds lock poisoned")
                    .take()
                    .ok_or(crate::Error::DeferredScopeFailed { scope: S::ID })?;

                open_in(
                    deferred.view.clone(),
                    S::ID,
                    Arc::clone(&deferred.parent),
                    Builds::Entry(deferred.entry),
                    seeds,
                )
                .await
            })
            .await
    }

    /// Resolves `H` from the instance, opening its container first if it is deferred.
    ///
    /// # Errors
    ///
    /// Fails when the container fails to open or `H` fails to resolve.
    pub async fn extract<H: FromContainer>(&self) -> crate::Result<H> {
        let container = self.container().await?;

        container.extract::<H>().await.map_err(crate::Error::from)
    }
}
