use upwell_core::{DependencyDescriptor, EntryKey, Scope, ScopeId, Singleton};
use upwell_di::{DependencyConsumer, DependencyFault, ProviderSelectionModel};

use super::EntryDescriptor;
use crate::error::quoted;
use crate::registry::{AppRegistry, BoundConfigs, ConfigFault};
use crate::scope::PreparedScopeTopology;

/// A typed failure while preparing an application's entry points.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EntryError {
    /// More entries were registered than dense entry identities can index.
    #[error(
        "an application can register at most {} entries, but {count} were registered",
        u32::MAX
    )]
    TooManyEntries {
        /// The number of registered entries.
        count: usize,
    },

    /// Two registrations share one stable entry identity.
    #[error("entry '{key}' is registered more than once")]
    DuplicateKey {
        /// The duplicated stable entry identity.
        key: EntryKey,
    },

    /// An entry runs in a scope the active protocol does not open.
    #[error("entry '{key}' runs in scope '{scope}', which the active protocol does not open")]
    UndeclaredScope {
        /// The entry's stable identity.
        key: EntryKey,
        /// The undeclared stable scope identity.
        scope: ScopeId,
    },

    /// An entry runs in the transient scope, which has no openable instance.
    #[error("entry '{key}' runs in the transient scope, which no entry can open")]
    TransientScope {
        /// The entry's stable identity.
        key: EntryKey,
    },

    /// No component or provider supplies an entry's target.
    #[error("entry '{key}' injects '{dependency}', which no component or provider supplies")]
    MissingDependency {
        /// The entry's stable identity.
        key: EntryKey,
        /// The target's Rust type name.
        dependency: &'static str,
    },

    /// Every provider of an entry's target lives in a scope the entry cannot reach.
    #[error(
        "entry '{key}' injects '{dependency}', but its providers live in {}, which scope '{scope}' cannot reach",
        quoted(.provider_scopes)
    )]
    UnreachableDependency {
        /// The entry's stable identity.
        key: EntryKey,
        /// The target's Rust type name.
        dependency: &'static str,
        /// The scope the entry runs in.
        scope: ScopeId,
        /// The scope of every matching provider, in provider order.
        provider_scopes: Box<[ScopeId]>,
    },

    /// More than one reachable provider ties for an entry's single-valued target.
    #[error(
        "entry '{key}' injects '{dependency}', but providers {} tie for it; mark exactly one \
         `#[primary]`, or select one with a qualifier",
        quoted(.candidates)
    )]
    AmbiguousDependency {
        /// The entry's stable identity.
        key: EntryKey,
        /// The target's Rust type name.
        dependency: &'static str,
        /// The stable component IDs of the tied providers.
        candidates: Box<[String]>,
    },

    /// A deferred target that only transient providers could satisfy.
    #[error("entry '{key}' defers '{dependency}', which only transient providers supply")]
    DeferredTransientDependency {
        /// The entry's stable identity.
        key: EntryKey,
        /// The target's Rust type name.
        dependency: &'static str,
    },

    /// A forced-fresh target, or one of its eager dependencies, lives in a scope the entry
    /// cannot reach.
    #[error(
        "entry '{key}' rebuilds '{dependency}' fresh, but it needs scope '{scope}', which the entry cannot reach"
    )]
    FreshUnreachable {
        /// The entry's stable identity.
        key: EntryKey,
        /// The target's Rust type name.
        dependency: &'static str,
        /// The unreachable scope.
        scope: ScopeId,
    },

    /// A forced-fresh target has no factory to rebuild it with.
    #[error(
        "entry '{key}' rebuilds '{dependency}' fresh, but component '{component}' has no factory"
    )]
    FreshWithoutFactory {
        /// The entry's stable identity.
        key: EntryKey,
        /// The target's Rust type name.
        dependency: &'static str,
        /// The target's stable component ID.
        component: String,
    },

    /// A config target has no binding of its type at the requested path.
    #[error(
        "entry '{key}' injects config '{type_name}', which has no binding{}",
        .path.map_or_else(String::new, |path| format!(" at path '{path}'"))
    )]
    MissingConfig {
        /// The entry's stable identity.
        key: EntryKey,
        /// The config type's Rust type name.
        type_name: &'static str,
        /// The requested property path, or `None` for the sole-binding shorthand.
        path: Option<&'static str>,
    },

    /// A sole-binding config target is bound at more than one path.
    #[error(
        "entry '{key}' injects config '{type_name}', which is bound at paths {}; name one with \
         `#[config(\"..\")]`",
        quoted(.paths)
    )]
    AmbiguousConfig {
        /// The entry's stable identity.
        key: EntryKey,
        /// The config type's Rust type name.
        type_name: &'static str,
        /// Every bound path.
        paths: Box<[String]>,
    },
}

impl EntryError {
    /// The stable identity of the failing entry, when one entry is at fault.
    pub const fn key(&self) -> Option<EntryKey> {
        match self {
            Self::TooManyEntries { .. } => None,
            Self::DuplicateKey { key }
            | Self::UndeclaredScope { key, .. }
            | Self::TransientScope { key }
            | Self::MissingDependency { key, .. }
            | Self::UnreachableDependency { key, .. }
            | Self::AmbiguousDependency { key, .. }
            | Self::DeferredTransientDependency { key, .. }
            | Self::FreshUnreachable { key, .. }
            | Self::FreshWithoutFactory { key, .. }
            | Self::MissingConfig { key, .. }
            | Self::AmbiguousConfig { key, .. } => Some(*key),
        }
    }
}

/// Rejects duplicate keys in `entries`, which are sorted by key.
pub(super) fn validate_keys(entries: &[EntryDescriptor]) -> Result<(), EntryError> {
    let duplicate = entries
        .windows(2)
        .find(|pair| pair[0].key == pair[1].key)
        .map(|pair| pair[0].key);

    match duplicate {
        Some(key) => Err(EntryError::DuplicateKey { key }),
        None => Ok(()),
    }
}

/// Requires every entry to run in the root scope or a boundary the protocol declares.
pub(super) fn validate_scopes(
    entries: &[EntryDescriptor],
    topology: &PreparedScopeTopology,
) -> Result<(), EntryError> {
    for entry in entries {
        let scope = entry.scope;

        if scope.is_transient() {
            return Err(EntryError::TransientScope { key: entry.key });
        }

        if scope.id() != Singleton::ID && !topology.contains(&scope.id()) {
            return Err(EntryError::UndeclaredScope {
                key: entry.key,
                scope: scope.id(),
            });
        }
    }

    Ok(())
}

/// Requires every target of every entry to resolve from the entry's scope in the graph
/// `selection` describes, and every config target to have exactly one matching binding.
pub(crate) fn validate_targets(
    registry: &AppRegistry,
    selection: &ProviderSelectionModel,
    topology: &PreparedScopeTopology,
) -> crate::Result<()> {
    let configs = BoundConfigs::new(&registry.config_bindings);
    let can_reach =
        |consumer: ScopeId, dependency: ScopeId| topology.is_reachable(&consumer, &dependency);

    for entry in &registry.entries {
        for target in (entry.targets)() {
            let error = if target.config {
                configs
                    .fault(&target)
                    .map(|fault| config_error(entry, &target, fault))
            } else {
                selection
                    .dependency_fault(&target, consumer(entry), can_reach)?
                    .map(|fault| dependency_error(entry, &target, fault))
            };

            if let Some(error) = error {
                return Err(error.into());
            }
        }
    }

    Ok(())
}

fn consumer(entry: &EntryDescriptor) -> DependencyConsumer {
    DependencyConsumer {
        id: entry.key.as_str(),
        scope: entry.scope,
    }
}

fn dependency_error(
    entry: &EntryDescriptor,
    target: &DependencyDescriptor,
    fault: DependencyFault,
) -> EntryError {
    let key = entry.key;
    let dependency = (target.ty.type_name)();

    match fault {
        DependencyFault::Unreachable { scopes } => EntryError::UnreachableDependency {
            key,
            dependency,
            scope: entry.scope.id(),
            provider_scopes: scopes.into_boxed_slice(),
        },
        DependencyFault::Ambiguous { candidates } => EntryError::AmbiguousDependency {
            key,
            dependency,
            candidates: candidates.into_boxed_slice(),
        },
        DependencyFault::DeferredTransient => {
            EntryError::DeferredTransientDependency { key, dependency }
        }
        DependencyFault::FreshUnreachable { scope } => EntryError::FreshUnreachable {
            key,
            dependency,
            scope,
        },
        DependencyFault::FreshWithoutFactory { component } => EntryError::FreshWithoutFactory {
            key,
            dependency,
            component,
        },
        _ => EntryError::MissingDependency { key, dependency },
    }
}

fn config_error(
    entry: &EntryDescriptor,
    target: &DependencyDescriptor,
    fault: ConfigFault,
) -> EntryError {
    let key = entry.key;
    let type_name = (target.ty.type_name)();

    match fault {
        ConfigFault::Missing => EntryError::MissingConfig {
            key,
            type_name,
            path: target.qualifier,
        },
        ConfigFault::Ambiguous { paths } => EntryError::AmbiguousConfig {
            key,
            type_name,
            paths: paths.into_boxed_slice(),
        },
    }
}
