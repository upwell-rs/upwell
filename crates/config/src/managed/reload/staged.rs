//! One staged reload: the re-read sources, the changed bindings and their prepared swaps.

use std::any::TypeId;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use upwell_core::TypeDescriptor;
use upwell_hooks::{HookKind, HookManager};

use super::{
    ChangedBinding, ComponentHookReport, ConfigReload, ConfigReloadError, ConfigReloader,
    PreparedSwap, ReloadProposal, StagedConfig,
};
use crate::ConfigValue;
use crate::condition::ResolvedConfig;
use crate::managed::{ConfigError, ConfigManager, ConfigStore};

/// A prepared-but-uncommitted reload: the changed bindings, the staged proposal values,
/// the committable swaps, and the generation-local candidate store. Produced by
/// [`ConfigReloader::stage`]; nothing is published until [`commit`](Self::commit) is
/// called.
pub struct StagedReload {
    pub(super) changed: Vec<ChangedBinding>,
    pub(super) staged: Vec<StagedConfig>,
    pub(super) prepared: Vec<PreparedSwap>,
    /// The generation-local candidate store, seeded on the first
    /// [`candidate_store`](Self::candidate_store) call from `staged` — never during
    /// staging itself, so a reload that never reads the proposal never seeds it.
    pub(super) candidate: OnceLock<Arc<ConfigStore>>,
    pub(super) new_root: ConfigValue,
    pub(super) reloader: ConfigReloader,
}

impl StagedReload {
    /// The bindings whose source changed and are ready to re-publish.
    pub fn changed(&self) -> &[ChangedBinding] {
        &self.changed
    }

    /// Renders the candidate tree's subtree at `path` for a component condition's raw
    /// `resolve` read, or `None` when the path is absent.
    pub fn resolve_raw(&self, path: &str) -> Result<Option<ResolvedConfig>, ConfigError> {
        self.reloader
            .lock_manager()
            .resolve_raw_in(&self.new_root, path)
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
    pub(super) fn candidate_is_built(&self) -> bool {
        self.candidate.get().is_some()
    }

    /// Whether no binding changed. An empty staged reload has nothing to commit.
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty()
    }

    /// Publishes every staged swap into its live slot, adopts the re-read tree, and
    /// advances the reloader generation, returning the new generation.
    ///
    /// The generation advance lives here — not with the caller — so every commit
    /// observes exactly one increment.
    pub fn commit(self) -> u64 {
        let mut manager = self.reloader.lock_manager();

        for swap in self.prepared {
            (swap.commit)();
        }

        manager.adopt(self.new_root);

        self.reloader.advance_generation()
    }

    /// Fires the config-reload event against `hooks` for every hook that targets a changed
    /// binding (or is routed `always`), returning the accepted participant outcomes in
    /// registration order. Participants run sequentially and the first error aborts the
    /// whole reload with [`ConfigReloadError::Hook`]; later participants and observers do
    /// not run. Observers run once every participant accepted, and their failures are
    /// logged without affecting the reload.
    ///
    /// `owner_id` names the component owning each hook: reports and errors carry the ID it
    /// returns for the hook's owner type, so the caller decides the naming scheme.
    ///
    /// The application runtime's transactional reload runs it against the candidate
    /// generation's manager before commit, naming owners by their stable component IDs.
    #[allow(clippy::result_large_err)]
    pub async fn run_config_reload_hooks(
        &self,
        hooks: &HookManager,
        owner_id: impl Fn(&TypeDescriptor) -> &'static str,
    ) -> Result<Vec<ComponentHookReport>, ConfigReloadError> {
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
        let fired = hooks
            .fire::<ConfigReload>(&proposal, |hook| {
                hook_targets_changed(hook, &changed_paths, &bindings_by_type)
            })
            .await;
        let mut reports = Vec::with_capacity(fired.participants.len());

        for (component, result) in fired.participants {
            match result {
                Ok(outcome) => reports.push(ComponentHookReport {
                    component: owner_id(&component),
                    outcome,
                }),

                Err(source) => {
                    return Err(ConfigReloadError::Hook {
                        component: owner_id(&component),
                        source: Box::new(source),
                    });
                }
            }
        }

        for (component, result) in fired.observers {
            if let Err(error) = result {
                tracing::warn!(
                    target: "upwell::config",
                    hook = ConfigReload::NAME,
                    component = owner_id(&component),
                    %error,
                    "config-reload observer failed"
                );
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
