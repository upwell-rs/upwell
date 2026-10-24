//! The injected config handles: the live `Cfg<T>` and the staged `CfgNext<T>`.

use std::sync::{Arc, Mutex};

use upwell_di::{Injectable, Live, LiveRef};

use super::source;

/// An injected configuration value, bound from a property path.
///
/// Backed by a shared [`Live<T>`] slot, so a config reload swaps the value in place
/// and every holder observes it on the next read; snapshots taken earlier stay
/// pinned. The path is supplied at the injection site via `#[config("...")]` (or
/// omitted for the sole-binding shorthand). Read with [`get`](Self::get) (a guard)
/// or [`snapshot`](Self::snapshot) (an owned `Arc`).
pub struct Cfg<T> {
    live: Live<T>,
    path: Arc<str>,
    committed_snapshot: Arc<Mutex<source::ConfigSnapshot>>,
}

impl<T: Send + Sync + 'static> Cfg<T> {
    /// Wraps a freshly bound value with the property path it was bound at.
    pub(crate) fn new(
        value: T,
        path: impl Into<Arc<str>>,
        committed_snapshot: source::ConfigSnapshot,
    ) -> Self {
        Self::from_shared(Arc::new(value), path, committed_snapshot)
    }

    /// Wraps an already-shared value with the property path it was bound at — the seam
    /// for seeding a generation-local candidate handle from a staged snapshot without
    /// re-serializing. Each call creates an independent live cell.
    pub(crate) fn from_shared(
        value: Arc<T>,
        path: impl Into<Arc<str>>,
        committed_snapshot: source::ConfigSnapshot,
    ) -> Self {
        Self {
            live: Live::new(value),
            path: path.into(),
            committed_snapshot: Arc::new(Mutex::new(committed_snapshot)),
        }
    }

    /// A guard pinning the current value, dereferencing to `T`, for short reads.
    pub fn get(&self) -> LiveRef<'_, T> {
        self.live.get()
    }

    /// An owned `Arc` snapshot of the current value — stable once taken.
    pub fn snapshot(&self) -> Arc<T> {
        self.live.snapshot()
    }

    /// The property path this value was bound from.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Publishes a re-bound value into the slot. Used by a config reload at commit.
    #[allow(dead_code)]
    pub(crate) fn replace(&self, value: Arc<T>) {
        self.live.replace(value);
    }

    pub(crate) fn committed_snapshot(&self) -> source::ConfigSnapshot {
        self.committed_snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn replace_snapshot(&self, snapshot: source::ConfigSnapshot) {
        *self
            .committed_snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = snapshot;
    }
}

impl<T: Send + Sync + 'static> Clone for Cfg<T> {
    fn clone(&self) -> Self {
        Self {
            live: self.live.clone(),
            path: Arc::clone(&self.path),
            committed_snapshot: Arc::clone(&self.committed_snapshot),
        }
    }
}

impl<T: Send + Sync + 'static> Injectable for Cfg<T> {
    type Target = T;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

/// A **proposed** configuration value handed to a `#[hook(ConfigReload)]` method during a
/// reload, before it is committed. Read-only; derefs to `T`. If every hook accepts the
/// proposal the value is committed into its [`Cfg<T>`] slot, otherwise it is discarded.
pub struct CfgNext<T> {
    value: Arc<T>,
    path: Arc<str>,
}

impl<T> CfgNext<T> {
    pub(crate) fn new(value: Arc<T>, path: Arc<str>) -> Self {
        Self { value, path }
    }

    /// The proposed value.
    pub fn get(&self) -> &T {
        &self.value
    }

    /// An owned `Arc` of the proposed value.
    pub fn snapshot(&self) -> Arc<T> {
        Arc::clone(&self.value)
    }

    /// The property path this value was bound from.
    pub fn path(&self) -> &str {
        &self.path
    }
}

impl<T> Clone for CfgNext<T> {
    fn clone(&self) -> Self {
        Self {
            value: Arc::clone(&self.value),
            path: Arc::clone(&self.path),
        }
    }
}

impl<T> std::ops::Deref for CfgNext<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.value
    }
}
