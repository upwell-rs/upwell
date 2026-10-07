//! Manual configuration reloading, with two-phase hooks.
//!
//! [`ConfigReloader`] re-reads the [`ConfigManager`]'s sources, diffs each binding's
//! merged subtree against the live tree, and re-publishes **only** the bindings whose
//! source actually changed. The transaction is two-phase: every changed binding is
//! re-deserialized into a proposed value first, the affected `#[hook(ConfigReload)]`
//! hooks run against those proposals and may **abort** the reload, and only if every
//! hook accepts are the new values committed into their shared
//! `Live` slots. On any failure nothing is published.

use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use upwell_core::{Cardinality, DependencyDescriptor, TypeDescriptor};
use upwell_di::{BoxedComponent, Injectable};
use upwell_hooks::{HookKind, HookManager, HookParam};

use crate::ConfigValue;

use super::{Cfg, CfgNext, ConfigError, ConfigManager, ConfigProperties, ConfigStore};

/// The config-reload hook kind: a `#[hook(ConfigReload)]` method runs when a config it
/// targets is reloaded, receiving the proposed value(s) as [`CfgNext<T>`] and returning a
/// [`HookOutcome`].
pub struct ConfigReload;

impl HookKind for ConfigReload {
    type Output = HookOutcome;
    type Cx = ReloadProposal;

    const NAME: &'static str = "config_reload";
}

/// What a `#[hook(ConfigReload)]` hook reports back. `Err` from the hook aborts the reload;
/// these variants all mean the proposal was accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookOutcome {
    /// The hook inspected the new config and made no change.
    Unchanged,
    /// The hook applied the new config to its internal state.
    Reloaded,
    /// The config is valid but cannot be applied at runtime; a restart is required.
    RestartRequired(&'static str),
}

/// The proposed configuration handed to config-reload hooks: the staged value of every
/// binding (changed bindings hold their newly-deserialized value, unchanged bindings hold
/// their current value), so a hook's [`CfgNext<T>`] params always resolve.
pub struct ReloadProposal {
    staged: Vec<StagedConfig>,
}

/// One binding's value staged into a [`ReloadProposal`] — its type, path, erased value,
/// and an internal factory seeding the binding's generation-local candidate `Cfg`.
#[derive(Clone)]
pub struct StagedConfig {
    type_id: TypeId,
    path: Arc<str>,
    value: Arc<dyn Any + Send + Sync>,
    /// Type-erased candidate seed, built where the binding's type is known: each call
    /// creates a fresh live cell (never aliasing an active one) holding the staged
    /// value with the snapshot the binding would hold after commit. Carried here rather
    /// than on [`ReloadableConfig`] because `StagedConfig` cannot be constructed
    /// downstream, so a trait method taking or returning it would be unimplementable
    /// for external implementers.
    seed: Arc<dyn Fn() -> BoxedComponent + Send + Sync>,
}

impl StagedConfig {
    /// The staged binding's type.
    pub fn type_id(&self) -> TypeId {
        self.type_id
    }

    /// The staged binding's property path.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The erased staged value. Values stored in an `Arc<T>` are unsized into
    /// `Arc<dyn Any>`, so the erased target remains `T` for both changed and unchanged
    /// bindings.
    pub fn value(&self) -> &dyn Any {
        self.value.as_ref()
    }

    /// Seeds a fresh, generation-local candidate `Cfg` for this binding. Crate-internal:
    /// the typed factory is created by the reloadable slot (which knows the binding's
    /// type) and travels with the staged entry.
    pub(crate) fn candidate_seed(&self) -> BoxedComponent {
        (self.seed)()
    }
}

impl ReloadProposal {
    /// Builds a proposal from staged values. Public so the transactional config-and-graph
    /// reload can run hooks over a proposal it staged outside [`ConfigReloader::reload`].
    pub fn new(staged: Vec<StagedConfig>) -> Self {
        Self { staged }
    }

    /// The proposed value of type `T` at `path` (or its sole binding when `path` is
    /// `None`), as a [`CfgNext<T>`]. `None` if no such binding is staged or the by-type
    /// lookup is ambiguous.
    fn next<T: Send + Sync + 'static>(&self, path: Option<&str>) -> Option<CfgNext<T>> {
        let type_id = TypeId::of::<T>();

        let entry = match path {
            Some(path) => self
                .staged
                .iter()
                .find(|staged| staged.type_id == type_id && &*staged.path == path)?,

            None => {
                let mut matches = self
                    .staged
                    .iter()
                    .filter(|staged| staged.type_id == type_id);
                let first = matches.next()?;

                if matches.next().is_some() {
                    return None;
                }

                first
            }
        };

        let value = entry.value.clone().downcast::<T>().ok()?;

        Some(CfgNext::new(value, entry.path.clone()))
    }
}

impl<T: ConfigProperties> HookParam<ConfigReload> for CfgNext<T> {
    fn dependency(path: Option<&'static str>) -> DependencyDescriptor {
        DependencyDescriptor {
            name: T::NAME,
            ty: TypeDescriptor::of::<T>(T::NAME),
            cardinality: Cardinality::One,
            optional: false,
            dynamic: false,
            qualifier: path,
            config: true,
            resolution: upwell_core::ResolutionMode::Eager,
            observation: upwell_core::DependencyObservation::Snapshot,
        }
    }

    fn extract(cx: &ReloadProposal, path: Option<&'static str>) -> upwell_hooks::Result<Self> {
        cx.next::<T>(path)
            .ok_or(upwell_hooks::Error::MissingParam(T::NAME))
    }
}

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
    pub component: &'static str,
    pub outcome: HookOutcome,
}

/// The outcome of a successful reload.
#[derive(Debug, Clone)]
pub struct ConfigReloadReport {
    /// Monotonic counter incremented on every successful reload.
    pub generation: u64,
    /// The bindings whose source changed and were re-published. Empty when nothing
    /// changed.
    pub changed: Vec<ChangedBinding>,
    /// The config-reload hooks that ran and accepted, with their outcomes.
    pub hooks: Vec<ComponentHookReport>,
}

/// A re-deserialized value ready to publish into its slot, plus the complete staged
/// proposal entry it contributes — carrying the binding's candidate seed — and the
/// committable swap. Produced in the reload's prepare step; committed only after every
/// hook accepts.
pub struct PreparedSwap {
    staged: StagedConfig,
    commit: Box<dyn FnOnce() + Send + Sync>,
}

/// A config binding the reloader can re-bind: it knows its path, how to re-deserialize its
/// type from a re-read tree, and how to stage its current value. Object-safe so the
/// reloader holds a `Vec<Box<dyn ReloadableConfig>>` across all bound types.
pub trait ReloadableConfig: Send + Sync {
    /// The property path this binding was bound at.
    fn path(&self) -> &str;

    /// The bound type's display name, for reports and errors.
    fn type_name(&self) -> &'static str;

    /// The bound type's `TypeId`, for matching hook params to changed bindings.
    fn type_id(&self) -> TypeId;

    /// Re-deserializes the binding from `new_root` and returns a committable swap.
    /// Fallible — a deserialize error aborts the whole reload before any commit.
    fn prepare(
        &self,
        manager: &ConfigManager,
        new_root: &ConfigValue,
    ) -> Result<Option<PreparedSwap>, ConfigError>;

    /// The current committed value, erased, for staging an unchanged binding into the
    /// proposal so hooks reading it still resolve.
    fn stage_current(&self) -> StagedConfig;
}

/// The reloadable record for one `Cfg<T>` binding: a clone of the injected handle
/// (sharing its `Live` slot) plus the property path.
pub(crate) struct ConfigSlot<T> {
    cfg: Cfg<T>,
    path: String,
}

impl<T: ConfigProperties> ConfigSlot<T> {
    /// Recovers a reloadable slot from a freshly bound config seed, sharing its live
    /// cell. Returns `None` if the seed does not hold a `Cfg<T>`.
    pub(crate) fn from_seed(
        _manager: &ConfigManager,
        seed: &BoxedComponent,
        path: &str,
    ) -> Option<Box<dyn ReloadableConfig>> {
        let cfg = seed.value.downcast_ref::<Cfg<T>>()?.clone();

        Some(Box::new(ConfigSlot {
            cfg,
            path: path.to_string(),
        }))
    }
}

impl<T: ConfigProperties> ReloadableConfig for ConfigSlot<T> {
    fn path(&self) -> &str {
        &self.path
    }

    fn type_name(&self) -> &'static str {
        T::NAME
    }

    fn type_id(&self) -> TypeId {
        TypeId::of::<T>()
    }

    fn prepare(
        &self,
        manager: &ConfigManager,
        new_root: &ConfigValue,
    ) -> Result<Option<PreparedSwap>, ConfigError> {
        let committed_snapshot = self.cfg.committed_snapshot();
        let unchanged = manager.snapshot_matches(new_root, &self.path, &committed_snapshot);

        if unchanged {
            return Ok(None);
        }

        let (value, next_snapshot) =
            manager.get_config_in_with_snapshot::<T>(new_root, &self.path)?;
        let replacement = Arc::new(value);
        let staged: Arc<dyn Any + Send + Sync> = replacement.clone();
        let committed = replacement.clone();
        let path: Arc<str> = Arc::from(self.path.as_str());
        let seed_snapshot = next_snapshot.clone();
        let cfg = self.cfg.clone();

        let staged_config = StagedConfig {
            type_id: TypeId::of::<T>(),
            path: path.clone(),
            value: staged,
            seed: Arc::new(move || BoxedComponent {
                ty: TypeDescriptor::of::<T>(T::NAME),
                value: Box::new(Cfg::from_shared(
                    replacement.clone(),
                    path.clone(),
                    seed_snapshot.clone(),
                )),
            }),
        };

        Ok(Some(PreparedSwap {
            staged: staged_config,
            commit: Box::new(move || {
                cfg.replace(committed);
                cfg.replace_snapshot(next_snapshot);
            }),
        }))
    }

    fn stage_current(&self) -> StagedConfig {
        let value = self.cfg.snapshot();
        let path: Arc<str> = Arc::from(self.path.as_str());
        let snapshot = self.cfg.committed_snapshot();

        StagedConfig {
            type_id: TypeId::of::<T>(),
            path: path.clone(),
            value: value.clone(),
            seed: Arc::new(move || BoxedComponent {
                ty: TypeDescriptor::of::<T>(T::NAME),
                value: Box::new(Cfg::from_shared(
                    value.clone(),
                    path.clone(),
                    snapshot.clone(),
                )),
            }),
        }
    }
}

/// A cheap, cloneable, injectable handle that re-reads configuration on demand.
///
/// Seeded by the daemon as a framework singleton, so any component or handler can
/// inject it (`reloader: ConfigReloader`) and trigger a reload. Always available;
/// signal- and file-watch-driven reloads (configured on the [`ConfigManager`]) build
/// on the same [`reload`](Self::reload) entry point.
#[derive(Clone)]
pub struct ConfigReloader {
    inner: Arc<ReloaderInner>,
}

struct ReloaderInner {
    manager: Mutex<ConfigManager>,
    slots: Vec<Box<dyn ReloadableConfig>>,
    /// The hook manager legacy reloads run against: the startup generation's manager,
    /// replaced with the just-published generation's manager at every transactional
    /// commit. Cloned out per reload under the serialization lease; the lock is never
    /// held across an await.
    hooks: RwLock<HookManager>,
    generation: AtomicU64,
    /// Serializes whole reloads. The prepare and commit phases each take the `manager`
    /// lock briefly (and release it so hooks can `await`), so without this two concurrent
    /// reloads could interleave and commit a stale tree. Held across all phases.
    in_progress: tokio::sync::Mutex<()>,
}

impl ConfigReloader {
    /// Builds a reloader over the manager, the reloadable slots of every bound config
    /// (sharing their live cells), and the initial hook manager that fires reload
    /// hooks. A transactional commit replaces the manager with its generation's (see
    /// [`install_hook_manager`](Self::install_hook_manager)).
    pub fn new(
        manager: ConfigManager,
        slots: Vec<Box<dyn ReloadableConfig>>,
        hooks: HookManager,
    ) -> Self {
        Self {
            inner: Arc::new(ReloaderInner {
                manager: Mutex::new(manager),
                slots,
                hooks: RwLock::new(hooks),
                generation: AtomicU64::new(0),
                in_progress: tokio::sync::Mutex::new(()),
            }),
        }
    }

    /// The number of successful reloads so far (the current generation).
    pub fn generation(&self) -> u64 {
        self.inner.generation.load(Ordering::SeqCst)
    }

    /// Advances the reload generation, returning the new value.
    fn advance_generation(&self) -> u64 {
        self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Snapshots the current hook manager. Cheap (an `Arc` clone) and synchronous, so
    /// the internal lock is never held across an await; callers take the snapshot under
    /// the reload serialization lease, which excludes a concurrent transactional commit
    /// replacing the manager mid-reload.
    fn current_hook_manager(&self) -> HookManager {
        self.inner
            .hooks
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Replaces the hook manager legacy reloads run against. Synchronous and infallible.
    ///
    /// Framework-internal seam: the app runtime's terminal commit installs the
    /// just-published generation's manager while both of its serializers are held, so
    /// direct reloads and watch/signal triggers run the current generation's hooks
    /// against the current root.
    #[doc(hidden)]
    pub fn install_hook_manager(&self, hooks: HookManager) {
        *self
            .inner
            .hooks
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = hooks;
    }

    /// A snapshot of the config source files, in merge order — the inputs a file watcher
    /// observes to drive [`reload`](Self::reload).
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

    /// Re-reads the config sources and re-publishes the changed bindings.
    ///
    /// Re-merges all sources in their original order (so profile precedence is preserved),
    /// diffs each binding's subtree, deserializes the changed ones into proposals, runs the
    /// affected `#[hook(ConfigReload)]` hooks, and — if every binding re-binds and every
    /// hook accepts — commits the new values into their shared slots. On any failure
    /// nothing is published and the live values are untouched.
    ///
    /// Hooks run against a snapshot of the current hook manager taken under the reload
    /// serialization lease — the manager the last transactional commit installed — so a
    /// newly activated generation's hooks run and removed generations' hooks do not.
    #[allow(clippy::result_large_err)]
    pub async fn reload(&self) -> Result<ConfigReloadReport, ConfigReloadError> {
        // Serialize whole reloads so the prepare → hooks → commit phases are atomic with
        // respect to each other: a concurrent reload cannot commit a newer tree between this
        // one's prepare and commit and have it overwritten by this one's stale `adopt`.
        let _in_progress = self.inner.in_progress.lock().await;

        // Snapshot the current manager under the lease, then release the internal lock
        // before any await. If nothing listens for config_reload, skip building proposals
        // entirely (O(1)).
        let hook_manager = self.current_hook_manager();
        let run_hooks = hook_manager.has::<ConfigReload>();

        let staged = self.stage_with(run_hooks)?;
        let changed = staged.changed.clone();

        // Nothing changed: no commit, no hooks — but a successful reload still advances the
        // generation so observers can tell a reload ran.
        if changed.is_empty() {
            let generation = self.advance_generation();

            return Ok(ConfigReloadReport {
                generation,
                changed,
                hooks: Vec::new(),
            });
        }

        // Phase 2 (hooks): run every config_reload hook that targets a changed path. Any
        // hook error aborts — the staged swaps are dropped, so nothing is committed.
        let hooks = staged.run_config_reload_hooks(&hook_manager).await?;

        // Phase 3 (commit): every binding re-bound and every hook accepted. The commit
        // publishes the swaps, adopts the re-read tree, and advances (and returns) the
        // reloader generation.
        let generation = staged.commit();

        Ok(ConfigReloadReport {
            generation,
            changed,
            hooks,
        })
    }

    /// Stages a reload without committing anything: re-reads the sources, diffs each
    /// binding's subtree, and deserializes changed bindings into committable swaps.
    ///
    /// The returned [`StagedReload`] is inert until [`StagedReload::commit`] is called,
    /// so a caller can run its own validation, hooks, or graph preparation between the
    /// two — over [`StagedReload::candidate_store`], which lazily re-seeds every
    /// binding into fresh cells without touching the active ones. The caller is
    /// responsible for serializing stage → commit against [`reload`](Self::reload)
    /// (e.g. by holding [`lock_reload`](Self::lock_reload) across the whole
    /// transaction).
    #[allow(clippy::result_large_err)]
    pub fn stage(&self) -> Result<StagedReload, ConfigReloadError> {
        self.stage_with(true)
    }

    /// Acquires the reload serialization lock. Hold it across an external
    /// stage → validate → commit transaction so a concurrent [`reload`](Self::reload)
    /// cannot interleave and commit a newer tree in between.
    pub async fn lock_reload(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.inner.in_progress.lock().await
    }

    #[allow(clippy::result_large_err)]
    fn stage_with(&self, stage_unchanged: bool) -> Result<StagedReload, ConfigReloadError> {
        // Phase 1 (prepare): re-read, diff, and deserialize changed bindings — all under
        // the manager lock, with no await, so the lock is released before hooks run. User
        // deserializers execute in this boundary; convert their panics to a failed reload.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let manager = self.lock_manager();
            let new_root = manager
                .reread()
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
                    // An unchanged binding joins the proposal only for callers that
                    // read it (the public `stage`); the legacy reload skips the work
                    // entirely. `stage_current` builds the staged metadata and its
                    // type-erased seed closure here, but the candidate `Cfg` seed
                    // value itself is only instantiated lazily by `candidate_store`.
                    if stage_unchanged {
                        staged.push(slot.stage_current());
                    }

                    continue;
                };

                if stage_unchanged {
                    staged.push(swap.staged.clone());
                }

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
}

/// A prepared-but-uncommitted reload: the changed bindings, the staged proposal values,
/// the committable swaps, and the generation-local candidate store. Produced by
/// [`ConfigReloader::stage`]; nothing is published until [`commit`](Self::commit) is
/// called.
pub struct StagedReload {
    changed: Vec<ChangedBinding>,
    staged: Vec<StagedConfig>,
    prepared: Vec<PreparedSwap>,
    /// The generation-local candidate store, seeded on the first
    /// [`candidate_store`](Self::candidate_store) call from `staged` — never during
    /// staging itself, so a reload that never reads the proposal never seeds it.
    candidate: OnceLock<Arc<ConfigStore>>,
    new_root: ConfigValue,
    reloader: ConfigReloader,
}

impl StagedReload {
    /// The bindings whose source changed and are ready to re-publish.
    pub fn changed(&self) -> &[ChangedBinding] {
        &self.changed
    }

    /// The staged typed entries of exactly the changed bindings, in binding order.
    ///
    /// Unlike [`changed`](Self::changed), each entry carries the binding's exact
    /// [`TypeId`], so consumers can match config types by identity rather than display
    /// name. Derived from the changed swaps; empty for an unchanged source.
    #[doc(hidden)]
    pub fn changed_staged(&self) -> impl ExactSizeIterator<Item = &StagedConfig> + '_ {
        self.prepared.iter().map(|swap| &swap.staged)
    }

    /// The staged value of every binding (changed bindings hold their newly-deserialized
    /// value, unchanged bindings hold their current value), for hooks and condition-fact
    /// extraction.
    pub fn staged(&self) -> &[StagedConfig] {
        &self.staged
    }

    /// The generation-local candidate store: every binding — changed and unchanged —
    /// re-seeded into fresh `Cfg` cells that do not alias the active ones, holding the
    /// values [`commit`](Self::commit) would publish. Seeded lazily on the first call,
    /// exactly once from the staged entries, so creation stays infallible and repeated
    /// calls return the same store.
    pub fn candidate_store(&self) -> Arc<ConfigStore> {
        self.candidate
            .get_or_init(|| {
                let mut candidate = ConfigStore::default();

                for entry in &self.staged {
                    candidate.insert(entry.path().to_string(), entry.candidate_seed());
                }

                Arc::new(candidate)
            })
            .clone()
    }

    /// Whether the candidate store has been built. Test-only seam: lets the crate's
    /// unit tests pin that construction is lazy (never during staging) and
    /// exactly-once.
    #[cfg(test)]
    fn candidate_is_built(&self) -> bool {
        self.candidate.get().is_some()
    }

    /// Whether no binding changed. An empty staged reload has nothing to commit.
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty()
    }

    /// Publishes every staged swap into its live slot, adopts the re-read tree, and
    /// advances the reloader generation, returning the new generation.
    ///
    /// The generation advance lives here — not with the caller — so every pipeline that
    /// commits a staged reload (the legacy [`ConfigReloader::reload`] and the app
    /// runtime's transactional reload) observes exactly one increment per commit.
    pub fn commit(self) -> u64 {
        let mut manager = self.reloader.lock_manager();

        for swap in self.prepared {
            (swap.commit)();
        }

        manager.adopt(self.new_root);

        self.reloader.advance_generation()
    }

    /// Runs every config-reload hook that targets a changed binding against `hooks`,
    /// returning the accepted outcomes in registration order. Any hook error aborts the
    /// whole reload with [`ConfigReloadError::Hook`].
    ///
    /// Shared by the legacy [`ConfigReloader::reload`] (against the active generation's
    /// manager) and the app runtime's transactional reload (against the candidate
    /// generation's manager), so both pipelines filter and execute hooks identically.
    #[allow(clippy::result_large_err)]
    pub async fn run_config_reload_hooks(
        &self,
        hooks: &HookManager,
    ) -> Result<Vec<ComponentHookReport>, ConfigReloadError> {
        // Nothing listens for config_reload: no proposal is built (O(1)).
        if !hooks.has::<ConfigReload>() {
            return Ok(Vec::new());
        }

        let changed_paths = self
            .changed
            .iter()
            .map(|binding| binding.path.clone())
            .collect::<HashSet<String>>();
        let bindings_by_type = bindings_by_type_index(&self.reloader.lock_manager());
        let proposal = ReloadProposal::new(self.staged.clone());
        let outcomes = hooks
            .run::<ConfigReload>(&proposal, |hook| {
                hook_targets_changed(hook, &changed_paths, &bindings_by_type)
            })
            .await;

        let mut reports = Vec::with_capacity(outcomes.len());

        for (component, result) in outcomes {
            match result {
                Ok(outcome) => reports.push(ComponentHookReport {
                    component: component.name,
                    outcome,
                }),

                Err(source) => {
                    return Err(ConfigReloadError::Hook {
                        component: component.name,
                        source: Box::new(source),
                    });
                }
            }
        }

        Ok(reports)
    }
}

/// Indexes bound config types to their property paths, so a hook's by-type (sole-binding)
/// `CfgNext<T>` param can be resolved to the path it targets.
fn bindings_by_type_index(manager: &ConfigManager) -> HashMap<TypeId, Vec<String>> {
    let mut index: HashMap<TypeId, Vec<String>> = HashMap::new();

    for binding in manager.bindings() {
        index
            .entry(binding.ty.type_id)
            .or_default()
            .push(binding.path.clone());
    }

    index
}

/// Whether a hook targets a changed path: any of its `CfgNext<T>` params resolves (by
/// `#[config("path")]` qualifier, or its type's sole binding) to a path that changed.
fn hook_targets_changed(
    hook: &upwell_hooks::HookDescriptor,
    changed_paths: &HashSet<String>,
    bindings_by_type: &HashMap<TypeId, Vec<String>>,
) -> bool {
    (hook.dependencies)().iter().any(|dep| {
        if !dep.config {
            return false;
        }

        match dep.qualifier {
            Some(path) => changed_paths.contains(path),

            None => match bindings_by_type.get(&dep.ty.type_id) {
                Some(paths) if paths.len() == 1 => changed_paths.contains(&paths[0]),
                _ => false,
            },
        }
    })
}

/// The stable component id of the seeded [`ConfigReloader`] singleton.
pub const CONFIG_RELOADER_ID: &str = "upwell:config-reloader";

/// The display name of the seeded [`ConfigReloader`] singleton.
pub const CONFIG_RELOADER_NAME: &str = "ConfigReloader";

impl upwell_di::Component for ConfigReloader {
    type Handle = ConfigReloader;

    const ID: &'static str = CONFIG_RELOADER_ID;
    const NAME: &'static str = CONFIG_RELOADER_NAME;

    fn into_handle(self) -> Self::Handle {
        self
    }
}

impl Injectable for ConfigReloader {
    type Target = ConfigReloader;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }

    fn snapshot_stored(stored: &Self) -> Option<Self> {
        Some(stored.clone())
    }
}

/// Under `di-check`, the reloader is framework-seeded, so it is always provided.
#[cfg(feature = "di-check")]
impl upwell_di::Provide<ConfigReloader> for upwell_di::Wiring {}

#[cfg(test)]
mod tests;
