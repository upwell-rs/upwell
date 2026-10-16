//! Config facts and config inputs supplied to one condition evaluation.

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use upwell_core::condition::scope::RawConfigValue;
use upwell_core::{ConditionInputId, ConditionScalar, ConfigBindingId, ConfigFactId};

use super::ConditionError;

/// Typed config facts and config inputs supplied to one catalog evaluation.
#[derive(Clone, Default)]
pub struct ConditionFactSnapshot {
    pub(super) facts: BTreeMap<ConfigFactId, ConditionScalar>,
    pub(super) config: BTreeMap<ConditionInputId, ConfigInput>,
}

impl ConditionFactSnapshot {
    pub fn new(
        facts: impl IntoIterator<Item = (ConfigFactId, ConditionScalar)>,
    ) -> Result<Self, ConditionError> {
        let mut snapshot = Self::default();

        for (id, value) in facts {
            if snapshot.facts.insert(id, value).is_some() {
                return Err(ConditionError::DuplicateFactValue(id));
            }
        }

        Ok(snapshot)
    }

    /// Supplies the staged value of one typed binding. A binding counts as changed between
    /// snapshots unless both hold the same shared value, so an unchanged binding must be
    /// supplied as its committed `Arc`.
    pub fn insert_binding(
        &mut self,
        id: ConfigBindingId,
        value: Arc<dyn Any + Send + Sync>,
    ) -> Result<(), ConditionError> {
        self.insert_config(ConditionInputId::Binding(id), ConfigInput::Binding(value))
    }

    /// Supplies the resolved subtree at one raw path, `None` when the path is absent.
    /// Raw inputs compare by value between snapshots.
    pub fn insert_raw<T: Any + Send + Sync + PartialEq>(
        &mut self,
        path: &'static str,
        value: Option<T>,
    ) -> Result<(), ConditionError> {
        let input = ConfigInput::Raw {
            value: value.map(|value| Arc::new(value) as RawConfigValue),
            same: same_raw::<T>,
        };

        self.insert_config(ConditionInputId::Raw(path), input)
    }

    fn insert_config(
        &mut self,
        id: ConditionInputId,
        input: ConfigInput,
    ) -> Result<(), ConditionError> {
        if self.config.insert(id, input).is_some() {
            return Err(ConditionError::DuplicateConfigInput(id));
        }

        Ok(())
    }

    /// The config inputs whose value differs from `previous`, among `declared`.
    pub(super) fn changed_config(
        &self,
        previous: &Self,
        declared: &BTreeSet<ConditionInputId>,
    ) -> Vec<ConditionInputId> {
        declared
            .iter()
            .filter(|id| match (self.config.get(id), previous.config.get(id)) {
                (Some(current), Some(previous)) => !current.same(previous),
                (None, None) => false,
                _ => true,
            })
            .copied()
            .collect()
    }
}

impl fmt::Debug for ConditionFactSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let facts = self
            .facts
            .iter()
            .map(|(id, value)| (*id, value.kind()))
            .collect::<Vec<_>>();
        let config = self.config.keys().collect::<Vec<_>>();

        formatter
            .debug_struct("ConditionFactSnapshot")
            .field("facts", &facts)
            .field("config", &config)
            .finish()
    }
}

/// One supplied config input: a staged typed binding or a resolved raw subtree.
#[derive(Clone)]
pub(super) enum ConfigInput {
    Binding(Arc<dyn Any + Send + Sync>),
    Raw {
        value: Option<RawConfigValue>,
        same: fn(&(dyn Any + Send + Sync), &(dyn Any + Send + Sync)) -> bool,
    },
}

impl ConfigInput {
    pub(super) fn value(&self) -> Option<&(dyn Any + Send + Sync)> {
        match self {
            Self::Binding(value) => Some(value.as_ref()),
            Self::Raw { value, .. } => value.as_deref(),
        }
    }

    pub(super) fn raw(&self) -> Option<RawConfigValue> {
        match self {
            Self::Binding(_) => None,
            Self::Raw { value, .. } => value.clone(),
        }
    }

    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Binding(left), Self::Binding(right)) => Arc::ptr_eq(left, right),

            (
                Self::Raw {
                    value: Some(left),
                    same,
                },
                Self::Raw {
                    value: Some(right), ..
                },
            ) => same(left.as_ref(), right.as_ref()),

            (Self::Raw { value: None, .. }, Self::Raw { value: None, .. }) => true,

            _ => false,
        }
    }
}

fn same_raw<T: Any + PartialEq>(
    left: &(dyn Any + Send + Sync),
    right: &(dyn Any + Send + Sync),
) -> bool {
    matches!(
        (left.downcast_ref::<T>(), right.downcast_ref::<T>()),
        (Some(left), Some(right)) if left == right
    )
}
