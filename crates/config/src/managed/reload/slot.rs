//! The reloadable slot behind each config binding: staging a fresh value and committing it into the shared `Live` cell.

use std::any::{Any, TypeId};
use std::sync::Arc;

use upwell_core::TypeDescriptor;
use upwell_di::BoxedComponent;

use super::{PreparedSwap, StagedConfig};
use crate::ConfigValue;
use crate::managed::{Cfg, ConfigError, ConfigManager, ConfigProperties};

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
