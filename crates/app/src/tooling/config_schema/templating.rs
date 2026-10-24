//! The `x-upwell-templating` descriptor: the `${..}` grammar and key resolution an IDE plugin
//! needs to complete and validate templates inside config values.

use serde_json::{Value, json};
use upwell_dirs::{Cache, Config, Data, DirKind, Runtime, State, Tmp};

/// The namespace prefix of directory keys (`${@data}`).
const DIRECTORY_NAMESPACE: &str = "@";

/// Describes the template syntax and key resolution, naming every bound config path as a
/// completion candidate.
pub(super) fn descriptor(binding_paths: &[String]) -> Value {
    let directories = [
        Config::LABEL,
        Data::LABEL,
        Cache::LABEL,
        State::LABEL,
        Runtime::LABEL,
        Tmp::LABEL,
    ]
    .map(|label| format!("{DIRECTORY_NAMESPACE}{label}"));

    json!({
        "placeholder": "${key}",
        "placeholder-with-default": "${key:default}",
        "escape": "$$",
        "full-placeholder-typed": true,
        "resolution": [
            { "keys": "dotted", "example": "server.port", "order": ["config-path", "environment"] },
            { "keys": "screaming-snake-case", "example": "DATABASE_URL", "order": ["environment", "config-path"] },
            { "keys": "other", "example": "port", "order": ["config-path", "environment"] },
            { "keys": "namespace", "example": "@data", "order": ["namespace"] },
        ],
        "namespaces": [
            { "prefix": DIRECTORY_NAMESPACE, "resolver": "directories", "keys": directories },
        ],
        "config-paths": binding_paths,
    })
}
