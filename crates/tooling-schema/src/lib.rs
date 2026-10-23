//! Versioned protocol-neutral documents exchanged with Upwell developer tooling.
//!
//! The schema contains only stable textual identities and JSON data. It deliberately has no
//! dependency on application runtime state, Clap, Cargo metadata, or a concrete protocol.

use std::collections::{BTreeMap, BTreeSet};

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

mod cli;
mod diagnostic;
mod document;
mod identity;
mod preview;
mod probe;
mod resource;

pub use cli::*;
pub use diagnostic::*;
pub use document::*;
pub use identity::*;
pub use preview::*;
pub use probe::*;
pub use resource::*;

/// JSON value used by opaque protocol and plugin facets.
pub use serde_json::Value as JsonValue;

/// Tooling schema version published by this package.
pub const TOOLING_SCHEMA_VERSION: Version = parse_package_version(env!("CARGO_PKG_VERSION"));

/// Exact hidden process argument used to request one target-local tooling probe.
pub const TOOLING_PROBE_ARGUMENT: &str = "--__upwell-tooling-probe-v1";

/// Environment variable containing the response file path for a target-local tooling probe.
pub const TOOLING_PROBE_OUTPUT_ENV: &str = "UPWELL_TOOLING_PROBE_OUTPUT";

/// Environment variable containing the Cargo package name selected by the tooling invoker.
pub const TOOLING_PROBE_PACKAGE_NAME_ENV: &str = "UPWELL_TOOLING_PROBE_PACKAGE_NAME";

/// Environment variable containing the selected Cargo package version.
pub const TOOLING_PROBE_PACKAGE_VERSION_ENV: &str = "UPWELL_TOOLING_PROBE_PACKAGE_VERSION";

/// Environment variable containing the selected Cargo package manifest path.
pub const TOOLING_PROBE_MANIFEST_PATH_ENV: &str = "UPWELL_TOOLING_PROBE_MANIFEST_PATH";

/// Environment variable containing the Cargo binary target selected by the tooling invoker.
pub const TOOLING_PROBE_BINARY_NAME_ENV: &str = "UPWELL_TOOLING_PROBE_BINARY_NAME";

/// Document facet holding the JSON Schema (2020-12) of the whole configuration tree, with every
/// binding nested at its path and the template grammar under `x-upwell-templating`.
pub const CONFIG_SCHEMA_FACET: &str = "framework/config-schema";

/// Resource-local suffix of the facet holding one config binding's JSON Schema; the full facet
/// id is `<binding resource id>/config-schema`.
pub const CONFIG_BINDING_SCHEMA_FACET: &str = "config-schema";

/// Schema version of the [`CONFIG_SCHEMA_FACET`] and [`CONFIG_BINDING_SCHEMA_FACET`] payloads.
pub const CONFIG_SCHEMA_FACET_VERSION: u16 = 1;

const fn parse_package_version(version: &str) -> Version {
    let bytes = version.as_bytes();
    let mut components = [0_u64; 3];
    let mut component = 0_usize;
    let mut has_digit = false;
    let mut index = 0_usize;

    while index < bytes.len() {
        let byte = bytes[index];

        if byte >= b'0' && byte <= b'9' {
            components[component] = match components[component].checked_mul(10) {
                Some(value) => value,
                None => panic!("tooling schema package version component overflows u64"),
            };
            components[component] = match components[component].checked_add((byte - b'0') as u64) {
                Some(value) => value,
                None => panic!("tooling schema package version component overflows u64"),
            };
            has_digit = true;
        } else if byte == b'.' {
            assert!(
                has_digit && component < 2,
                "tooling schema package version must contain major, minor, and patch components"
            );

            component += 1;
            has_digit = false;
        } else {
            panic!("tooling schema package version must not contain prerelease or build metadata");
        }

        index += 1;
    }

    assert!(
        component == 2 && has_digit,
        "tooling schema package version must contain major, minor, and patch components"
    );

    Version::new(components[0], components[1], components[2])
}

fn schema_requirement() -> VersionReq {
    VersionReq::parse(&format!(
        ">={TOOLING_SCHEMA_VERSION},<{}.0.0",
        TOOLING_SCHEMA_VERSION.major + 1
    ))
    .expect("the package-derived tooling schema requirement is valid")
}

fn is_namespaced(value: &str) -> bool {
    let mut parts = value.split('/');

    matches!((parts.next(), parts.next()), (Some(namespace), Some(name)) if !namespace.is_empty() && !name.is_empty())
}

fn is_blank(value: &str) -> bool {
    value.trim().is_empty()
}

#[cfg(test)]
mod tests;
