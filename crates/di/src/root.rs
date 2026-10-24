//! The framework-seeded [`RootResolver`] singleton: run-time access to the root container.
//!
//! Some framework singletons need to resolve components or extract dependencies from the
//! DI container *after* the app is built — a background scheduler firing job methods, a
//! task supervisor, and so on. They cannot inject a strong `Arc<ScopeContainer>` at
//! construction: the root container stores them, so a strong back-reference would be a
//! permanent reference cycle, and — like the container's own [`ComponentSource`] — the
//! self-handle only exists once the root is frozen.
//!
//! [`RootResolver`] is the general primitive for that. It is seeded as an ordinary
//! by-value injectable *before* the root is built (its slot empty) and [`attach`]ed to the
//! finished root afterwards, exactly as the [`HookManager`](upwell_hooks::HookManager) is.
//! It holds only a [`Weak`] to the root, so it adds no cycle; a consumer upgrades it per
//! call. This lets any singleton reach the whole [`FromContainer`] surface at run time
//! without the container layer knowing what it is used for.
//!
//! [`attach`]: RootResolver::attach
//! [`ComponentSource`]: crate::container::ComponentSource

use std::sync::{Arc, OnceLock, Weak};

use upwell_core::TypeDescriptor;

use crate::construct::FromContainer;
use crate::container::ScopeContainer;
use crate::descriptors::{Component, Injectable};
use crate::error::Error;

/// The stable component id of the seeded [`RootResolver`] singleton.
pub const ROOT_RESOLVER_ID: &str = "upwell:root-resolver";

/// The display name of the seeded [`RootResolver`] singleton.
pub const ROOT_RESOLVER_NAME: &str = "RootResolver";

/// A cheap-clone handle to the root scope container, for run-time resolution.
///
/// Seeded as a framework singleton, so any component can inject it by value. The backing
/// root is [`attach`](Self::attach)ed once, after build; before that (and after the root is
/// dropped on shutdown) resolution fails with [`Error::RootUnavailable`]. Every clone shares
/// one slot, so an attach is observed by all of them.
#[derive(Clone, Default)]
pub struct RootResolver {
    root: Arc<OnceLock<Weak<ScopeContainer>>>,
}

impl RootResolver {
    /// A fresh, unattached handle. Seeded as the singleton instance before the root exists.
    pub fn new() -> Self {
        Self::default()
    }

    /// Attaches the finished root container. Idempotent; a second attach is ignored. Stored
    /// as a [`Weak`] so the handle never keeps the root alive.
    pub fn attach(&self, root: &Arc<ScopeContainer>) {
        let _ = self.root.set(Arc::downgrade(root));
    }

    /// The live root, or `None` if it was never attached or has been dropped.
    fn root(&self) -> Option<Arc<ScopeContainer>> {
        self.root.get()?.upgrade()
    }

    /// Resolves the component of type `C` as its handle through the root scope.
    pub fn component<C: Component>(&self) -> crate::Result<C::Handle> {
        self.root()
            .ok_or(Error::RootUnavailable)?
            .get::<C>()
            .ok_or(Error::MissingComponent(C::NAME))
    }

    /// Extracts any [`FromContainer`] value from the root scope — the run-time analogue of a
    /// factory parameter, so a caller can resolve the same shapes a constructor's parameters
    /// can (`Arc<T>`, `Dep<T>`, `Cfg<T>`, `Vec`/`HashMap`/`Option` of providers, …).
    pub async fn extract<H: FromContainer>(&self) -> crate::Result<H> {
        let root = self.root().ok_or(Error::RootUnavailable)?;

        root.extract::<H>().await
    }
}

impl Component for RootResolver {
    type Handle = RootResolver;

    const ID: &'static str = ROOT_RESOLVER_ID;
    const NAME: &'static str = ROOT_RESOLVER_NAME;

    fn into_handle(self) -> Self::Handle {
        self
    }
}

impl Injectable for RootResolver {
    type Target = RootResolver;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

/// The descriptor for the framework-seeded [`RootResolver`] singleton.
pub const fn root_resolver_descriptor() -> crate::ComponentDescriptor {
    crate::ComponentDescriptor::manual(
        ROOT_RESOLVER_ID,
        ROOT_RESOLVER_NAME,
        TypeDescriptor::of::<RootResolver>(ROOT_RESOLVER_NAME),
        &upwell_core::Singleton,
    )
}

/// Under `di-check`, the root resolver is framework-seeded, so it is always provided.
#[cfg(feature = "di-check")]
impl crate::descriptors::Provided<RootResolver> for crate::descriptors::Wiring {}
