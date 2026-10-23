//! The `ConfigReload` hook kind and the staged proposal its hooks read.

use std::any::{Any, TypeId};
use std::sync::Arc;

use upwell_core::{Cardinality, DependencyDescriptor, TypeDescriptor};
use upwell_di::BoxedComponent;
use upwell_hooks::{HookKind, HookParam};

use crate::managed::{CfgNext, ConfigProperties};

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
    pub(super) staged: Vec<StagedConfig>,
}

/// One binding's value staged into a [`ReloadProposal`] — its type, path, erased value,
/// and an internal factory seeding the binding's generation-local candidate `Cfg`.
#[derive(Clone)]
pub struct StagedConfig {
    pub(super) type_id: TypeId,
    pub(super) path: Arc<str>,
    pub(super) value: Arc<dyn Any + Send + Sync>,
    /// Type-erased candidate seed, built where the binding's type is known: each call
    /// creates a fresh live cell (never aliasing an active one) holding the staged
    /// value with the snapshot the binding would hold after commit. Carried here rather
    /// than on [`ReloadableConfig`] because `StagedConfig` cannot be constructed
    /// downstream, so a trait method taking or returning it would be unimplementable
    /// for external implementers.
    pub(super) seed: Arc<dyn Fn() -> BoxedComponent + Send + Sync>,
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

    /// The staged value as a shared handle. An unchanged binding stages its committed
    /// value, so handle identity tells condition re-evaluation whether the binding changed.
    pub fn shared_value(&self) -> &Arc<dyn Any + Send + Sync> {
        &self.value
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
    /// Builds a proposal from staged values. Public so the application runtime's
    /// transactional reload can run hooks over a proposal it staged.
    pub fn new(staged: Vec<StagedConfig>) -> Self {
        Self { staged }
    }

    /// The proposed value of type `T` at `path` (or its sole binding when `path` is
    /// `None`), as a [`CfgNext<T>`]. `None` if no such binding is staged or the by-type
    /// lookup is ambiguous. Hook kinds whose context carries a proposal extract their
    /// `CfgNext<T>` parameters through it.
    pub fn next<T: Send + Sync + 'static>(&self, path: Option<&str>) -> Option<CfgNext<T>> {
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
