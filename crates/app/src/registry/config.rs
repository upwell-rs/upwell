//! Config-edge validation: every `#[config]` dependency names exactly one registered binding.

use std::any::TypeId;
use std::collections::HashMap;

use upwell_config::ConfigBinding;
use upwell_core::DependencyDescriptor;
use upwell_di::ComponentDescriptor;

use super::AppRegistry;
use crate::error::Error;

/// The registered config bindings, indexed by bound type.
pub(crate) struct BoundConfigs<'a> {
    paths: HashMap<TypeId, Vec<&'a str>>,
}

/// Why a config edge does not name exactly one binding.
pub(crate) enum ConfigFault {
    /// No binding of the edge's type exists at its path, or at all for the shorthand.
    Missing,
    /// The sole-binding shorthand matches bindings at more than one path.
    Ambiguous {
        /// Every bound path, in registration order.
        paths: Vec<String>,
    },
}

impl<'a> BoundConfigs<'a> {
    pub(crate) fn new(bindings: &'a [ConfigBinding]) -> Self {
        let mut paths: HashMap<TypeId, Vec<&str>> = HashMap::new();

        for binding in bindings {
            paths
                .entry(binding.ty.type_id)
                .or_default()
                .push(&binding.path);
        }

        Self { paths }
    }

    /// Checks one config edge: a `#[config("path")]` edge needs a binding of its type at
    /// that path, and the shorthand needs exactly one binding of its type.
    pub(crate) fn fault(&self, dependency: &DependencyDescriptor) -> Option<ConfigFault> {
        let paths = self
            .paths
            .get(&dependency.ty.type_id)
            .map_or(&[][..], Vec::as_slice);

        match (dependency.qualifier, paths) {
            (Some(path), _) if paths.contains(&path) => None,
            (Some(_), _) | (None, []) => Some(ConfigFault::Missing),
            (None, [_]) => None,
            (None, _) => Some(ConfigFault::Ambiguous {
                paths: paths.iter().map(|path| path.to_string()).collect(),
            }),
        }
    }
}

impl AppRegistry {
    /// Validates config edges against the registered bindings, as [`BoundConfigs::fault`]
    /// describes.
    pub(crate) fn validate_configs(&self, components: &[ComponentDescriptor]) -> crate::Result<()> {
        let configs = BoundConfigs::new(&self.config_bindings);

        for component in components {
            for dependency in component.dependencies().iter().filter(|dep| dep.config) {
                if let Some(fault) = configs.fault(dependency) {
                    return Err(config_error(component, dependency, fault));
                }
            }
        }

        Ok(())
    }
}

fn config_error(
    component: &ComponentDescriptor,
    dependency: &DependencyDescriptor,
    fault: ConfigFault,
) -> Error {
    let component = component.name.to_string();
    let type_name = (dependency.ty.type_name)().to_string();

    match fault {
        ConfigFault::Missing => Error::MissingConfig {
            component,
            type_name,
            path: dependency.qualifier.unwrap_or("<unqualified>").to_string(),
        },
        ConfigFault::Ambiguous { paths } => Error::AmbiguousConfig {
            component,
            type_name,
            count: paths.len(),
            paths: paths.join(", "),
        },
    }
}
