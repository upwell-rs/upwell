//! The configuration staging engine behind the application runtime's reload.
//!
//! [`ConfigReloader`] re-reads the [`ConfigManager`]'s sources, diffs each binding's
//! merged subtree against the live tree, and stages a proposal for every binding: only
//! the bindings whose source actually changed are re-deserialized into committable swaps,
//! while unchanged bindings contribute their current value so hooks reading them still
//! resolve. A [`StagedReload`] is inert: the application runtime's
//! transactional reload builds its candidate generation over the staged proposal, runs
//! the affected `#[hook(ConfigReload)]` hooks (which may **abort**), and commits the new
//! values into their shared `Live` slots only as part of its terminal publication. On
//! any failure nothing is published.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::{ConfigError, ConfigManager, ConfigOverlay};

mod hook;
mod slot;
mod staged;

pub use hook::{ConfigReload, HookOutcome, ReloadProposal, StagedConfig};
pub(crate) use slot::ConfigSlot;
pub use slot::ReloadableConfig;
pub use staged::StagedReload;

/// Errors from a configuration reload. On any error the live values are left
/// untouched — a reload commits only when every changed binding re-binds and every
/// affected hook accepts.
#[derive(Debug, thiserror::Error)]
pub enum ConfigReloadError {
    /// Re-reading or re-merging the config sources failed.
    #[error("failed to re-read configuration: {0}")]
    Load(#[source] ConfigError),

    /// A changed binding failed to deserialize from the new tree.
    #[error("failed to re-bind '{path}' as {type_name} during reload: {source}")]
    Bind {
        path: String,
        type_name: &'static str,
        #[source]
        source: ConfigError,
    },

    /// A `#[hook(ConfigReload)]` hook rejected the proposal; the reload was aborted.
    #[error("config_reload hook on '{component}' rejected the reload: {source}")]
    Hook {
        component: &'static str,
        #[source]
        source: Box<upwell_hooks::Error>,
    },

    /// User-provided deserialization panicked while preparing a reload. Panic
    /// payloads are omitted because they may contain application secrets.
    #[error("configuration reload panicked while preparing changed bindings")]
    Panicked,
}

/// One binding changed by a reload.
#[derive(Debug, Clone)]
pub struct ChangedBinding {
    pub path: String,
    pub type_name: &'static str,
}

/// One component's config-reload hook outcome.
#[derive(Debug, Clone)]
pub struct ComponentHookReport {
    /// The component owning the hook, as named by the caller of
    /// [`StagedReload::run_config_reload_hooks`]: its stable component ID in application
    /// runtime reports.
    pub component: &'static str,
    /// The outcome the hook returned.
    pub outcome: HookOutcome,
}

/// A re-deserialized value ready to publish into its slot, plus the complete staged
/// proposal entry it contributes — carrying the binding's candidate seed — and the
/// committable swap. Produced in the reload's prepare step; committed only after every
/// hook accepts.
pub struct PreparedSwap {
    staged: StagedConfig,
    commit: Box<dyn FnOnce() + Send + Sync>,
}

/// The config staging engine one application runtime drives.
///
/// It owns the managed config tree and the reloadable slots of every bound config, and
/// serializes reloads. It does not publish on its own: the application runtime's
/// transactional reload stages through it, prepares and validates a candidate
/// generation, and commits the staged values at its terminal publication. Request a
/// reload through the application's runtime reloader, not through this handle.
#[derive(Clone)]
pub struct ConfigReloader {
    inner: Arc<ReloaderInner>,
}

struct ReloaderInner {
    manager: Mutex<ConfigManager>,
    slots: Vec<Box<dyn ReloadableConfig>>,
    generation: AtomicU64,
    /// Serializes whole reloads. The prepare and commit phases each take the `manager`
    /// lock briefly (and release it so hooks can `await`), so without this two concurrent
    /// reloads could interleave and commit a stale tree. Held across all phases.
    in_progress: tokio::sync::Mutex<()>,
}

impl ConfigReloader {
    /// Builds a reloader over the manager and the reloadable slots of every bound config
    /// (sharing their live cells).
    pub fn new(manager: ConfigManager, slots: Vec<Box<dyn ReloadableConfig>>) -> Self {
        Self {
            inner: Arc::new(ReloaderInner {
                manager: Mutex::new(manager),
                slots,
                generation: AtomicU64::new(0),
                in_progress: tokio::sync::Mutex::new(()),
            }),
        }
    }

    /// The number of committed reloads so far (the current config generation).
    pub fn generation(&self) -> u64 {
        self.inner.generation.load(Ordering::SeqCst)
    }

    /// Advances the reload generation, returning the new value.
    fn advance_generation(&self) -> u64 {
        self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// A snapshot of the config source files, in merge order — the inputs a file watcher
    /// observes to trigger reloads.
    pub fn sources(&self) -> Vec<std::path::PathBuf> {
        self.lock_manager().sources().to_vec()
    }

    fn lock_manager(&self) -> std::sync::MutexGuard<'_, ConfigManager> {
        self.inner.manager.lock().unwrap_or_else(|poisoned| {
            tracing::warn!(
                target: "upwell::config",
                "recovering config manager after a panicking reload"
            );
            self.inner.manager.clear_poison();

            poisoned.into_inner()
        })
    }

    /// Stages a reload without committing anything: re-reads the sources, diffs each
    /// binding's subtree, and deserializes changed bindings into committable swaps.
    ///
    /// The returned [`StagedReload`] is inert until [`StagedReload::commit`] is called,
    /// so a caller can run its own validation, hooks, or graph preparation between the
    /// two — over [`StagedReload::candidate_store`], which lazily re-seeds every
    /// binding into fresh cells without touching the active ones. The caller is
    /// responsible for serializing stage → commit against other reloads by holding
    /// [`lock_reload`](Self::lock_reload) across the whole transaction.
    #[allow(clippy::result_large_err)]
    pub fn stage(&self) -> Result<StagedReload, ConfigReloadError> {
        self.stage_with(&ConfigOverlay::default())
    }

    /// Stages a reload like [`stage`](Self::stage), with `overlay` merged above every
    /// re-read source.
    ///
    /// This previews a proposed change without editing a source file: the staged
    /// proposal, its changed bindings, and its candidate store reflect the overlaid tree.
    /// Committing it adopts the overlaid tree until the next reload re-reads the sources.
    #[allow(clippy::result_large_err)]
    pub fn stage_with(&self, overlay: &ConfigOverlay) -> Result<StagedReload, ConfigReloadError> {
        // Phase 1 (prepare): re-read, diff, and deserialize changed bindings — all under
        // the manager lock, with no await, so the lock is released before hooks run. User
        // deserializers execute in this boundary; convert their panics to a failed reload.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let manager = self.lock_manager();
            let new_root = manager
                .reread(overlay)
                .map_err(|error| Box::new(ConfigReloadError::Load(error)))?;
            let mut prepared = Vec::new();
            let mut changed = Vec::new();
            let mut staged = Vec::new();

            for slot in &self.inner.slots {
                let swap = slot.prepare(&manager, &new_root).map_err(|source| {
                    Box::new(ConfigReloadError::Bind {
                        path: slot.path().to_string(),
                        type_name: slot.type_name(),
                        source,
                    })
                })?;

                let Some(swap) = swap else {
                    // `stage_current` builds the staged metadata and its type-erased seed
                    // closure here, but the candidate `Cfg` seed value itself is only
                    // instantiated lazily by `candidate_store`.
                    staged.push(slot.stage_current());

                    continue;
                };

                staged.push(swap.staged.clone());

                changed.push(ChangedBinding {
                    path: slot.path().to_string(),
                    type_name: slot.type_name(),
                });
                prepared.push(swap);
            }

            Ok::<_, Box<ConfigReloadError>>(StagedReload {
                changed,
                staged,
                prepared,
                candidate: OnceLock::new(),
                new_root,
                reloader: self.clone(),
            })
        }))
        .map_err(|_| ConfigReloadError::Panicked)?
        .map_err(|error| *error)
    }

    /// Acquires the reload serialization lock. Hold it across an external
    /// stage → validate → commit transaction so a concurrent reload cannot interleave
    /// and commit a newer tree in between.
    pub async fn lock_reload(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.inner.in_progress.lock().await
    }
}

#[cfg(test)]
mod tests;
