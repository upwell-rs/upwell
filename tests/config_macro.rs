//! End-to-end tests for the `#[config]` macro's field defaults and enum support,
//! exercised through `ConfigManager::get_config` with the directory namespace wired in.
//! These cover the full path: macro-emitted `defaults()` -> merge -> templated
//! resolution -> typed value.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use upwell::config::Toml;
use upwell::{
    ConditionFactSource, ConditionFacts, ConditionScalar, ConditionScalarKind,
    ConfigFactDescriptor, ConfigFactId, ConfigManager, DescriptorSource, DirectoriesManager,
    config,
};
use upwell_config::ResolverChain;

/// Resolves directory placeholders against a fixed root, so `${@runtime}` becomes
/// `<root>/runtime` deterministically.
fn manager(text: &str) -> ConfigManager {
    let dirs = DirectoriesManager::from_path(PathBuf::from("/base"));

    ConfigManager::<Toml>::from_str(text)
        .expect("parse config")
        .with_resolvers(ResolverChain::empty())
        .with_directories(&dirs)
        .into_dynamic()
}

/// Like [`manager`], but auto-discovers every `#[config(path)]` type and seeds their
/// defaults into the tree — so a default may reference another type's (or its own sibling's)
/// path even when that value is itself only a default.
fn seeded_manager(text: &str) -> ConfigManager {
    let dirs = DirectoriesManager::from_path(PathBuf::from("/base"));

    ConfigManager::<Toml>::from_str(text)
        .expect("parse config")
        .with_resolvers(ResolverChain::empty())
        .with_directories(&dirs)
        .auto_discover()
        .into_dynamic()
}

#[config]
#[derive(Debug, Deserialize)]
struct ServerCfg {
    port: u16,

    #[default = "localhost"]
    host: String,

    #[default = "${@runtime}/srv.sock"]
    socket: PathBuf,
}

#[test]
fn struct_defaults_fill_missing_fields_and_resolve_namespace() {
    // Only `port` is in the file; `host` and the directory-templated `socket` fall back
    // to their `#[default]`s.
    let config = manager("[server]\nport = 8080\n");

    let server: ServerCfg = config.get_config::<ServerCfg>("server").unwrap();

    assert_eq!(server.port, 8080);
    assert_eq!(server.host, "localhost");
    assert_eq!(server.socket, PathBuf::from("/base/runtime/srv.sock"));
}

#[test]
fn file_value_overrides_a_field_default() {
    let config = manager("[server]\nport = 80\nhost = \"db.internal\"\n");

    let server: ServerCfg = config.get_config::<ServerCfg>("server").unwrap();

    assert_eq!(server.host, "db.internal");
}

#[config]
#[derive(Debug, Deserialize, PartialEq)]
enum Storage {
    Memory,
    Disk {
        #[default = "${@data}/blobs"]
        path: PathBuf,
    },
}

#[test]
fn enum_variant_default_applies_to_selected_variant() {
    // `Disk` is selected with its `path` omitted, so the variant default fills it.
    let config = manager("[storage]\nDisk = {}\n");

    let storage: Storage = config.get_config::<Storage>("storage").unwrap();

    assert_eq!(
        storage,
        Storage::Disk {
            path: PathBuf::from("/base/data/blobs"),
        }
    );
}

#[test]
fn enum_unit_variant_round_trips() {
    let config = manager("storage = \"Memory\"\n");

    let storage: Storage = config.get_config::<Storage>("storage").unwrap();

    assert_eq!(storage, Storage::Memory);
}

#[config]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenamedCfg {
    // `rename_all` makes the serde key `maxRetries`; the default must land there.
    #[default = "3"]
    max_retries: u16,

    // An explicit field rename wins over `rename_all`.
    #[serde(rename = "sock")]
    #[default = "${@runtime}/x.sock"]
    socket: PathBuf,
}

#[test]
fn field_defaults_key_on_serde_renamed_names() {
    // Empty subtree: both defaults must materialize under their serde names, proving the
    // merge keys on `maxRetries` / `sock`, not the Rust identifiers.
    let config = manager("renamed = {}\n");

    let cfg: RenamedCfg = config.get_config::<RenamedCfg>("renamed").unwrap();

    assert_eq!(cfg.max_retries, 3);
    assert_eq!(cfg.socket, PathBuf::from("/base/runtime/x.sock"));
}

#[test]
fn renamed_field_value_from_file_overrides_default() {
    // The file supplies the serde-named key `maxRetries`; the default must not clobber it.
    let config = manager("[renamed]\nmaxRetries = 9\n");

    let cfg: RenamedCfg = config.get_config::<RenamedCfg>("renamed").unwrap();

    assert_eq!(cfg.max_retries, 9);
}

#[config]
#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", rename_all_fields = "camelCase")]
enum RenamedStorage {
    InMemory,
    OnDisk {
        #[default = "${@data}/blobs"]
        data_path: PathBuf,
    },
}

#[test]
fn enum_variant_and_field_renames_are_honored() {
    // `rename_all = snake_case` makes the tag `on_disk`; `rename_all_fields = camelCase`
    // makes the field `dataPath`. The default must key under both renamed names.
    let config = manager("[storage]\non_disk = {}\n");

    let storage: RenamedStorage = config.get_config::<RenamedStorage>("storage").unwrap();

    assert_eq!(
        storage,
        RenamedStorage::OnDisk {
            data_path: PathBuf::from("/base/data/blobs"),
        }
    );
}

#[config]
#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum DefaultedStorage {
    #[default]
    InMemory,
    OnDisk {
        #[default = "${@data}/blobs"]
        path: PathBuf,
    },
}

#[test]
fn enum_default_variant_used_when_section_is_empty() {
    // `[store]` is present but names no variant, so the `#[default]` unit variant
    // (`in_memory` after rename_all) is selected.
    let config = manager("[store]\n");

    let storage: DefaultedStorage = config.get_config::<DefaultedStorage>("store").unwrap();

    assert_eq!(storage, DefaultedStorage::InMemory);
}

#[test]
fn enum_default_variant_used_when_section_absent() {
    // The path is entirely absent; the default variant still materializes the value.
    let config = manager("");

    let storage: DefaultedStorage = config.get_config::<DefaultedStorage>("store").unwrap();

    assert_eq!(storage, DefaultedStorage::InMemory);
}

#[test]
fn enum_default_variant_overridden_by_explicit_selection() {
    // Explicitly choosing `on_disk` (path omitted → its field default) overrides the
    // `#[default]` variant.
    let config = manager("[store]\non_disk = {}\n");

    let storage: DefaultedStorage = config.get_config::<DefaultedStorage>("store").unwrap();

    assert_eq!(
        storage,
        DefaultedStorage::OnDisk {
            path: PathBuf::from("/base/data/blobs"),
        }
    );
}

#[config]
#[derive(Debug, Deserialize)]
struct SockOnly {
    #[default = "${@runtime}/srv.sock"]
    socket: PathBuf,
}

#[test]
fn load_from_registers_the_directory_namespace() {
    // `load_from` reads the (absent) config dir and wires `${@kind}` in one step, so a
    // `${@runtime}` default resolves without a separate `with_directories` call.
    let dirs = DirectoriesManager::from_path(PathBuf::from("/base"));
    let config =
        ConfigManager::<Toml>::load_from_with_resolvers(&dirs, &[], ResolverChain::empty())
            .expect("load config");

    let cfg: SockOnly = config.get_config::<SockOnly>("app").unwrap();

    assert_eq!(cfg.socket, PathBuf::from("/base/runtime/srv.sock"));
}

// An internally-tagged enum (`tag = "kind"`) with a default struct variant — the real-world
// shape that previously failed with "missing field `kind`".
#[config]
#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Transport {
    Tcp {
        #[default = "0.0.0.0"]
        addr: String,
        #[default = "2116"]
        port: u16,
    },
    #[default]
    Unix {
        #[default = "${@runtime}/d.sock"]
        socket: PathBuf,
    },
}

#[test]
fn internally_tagged_default_variant_synthesizes_the_tag() {
    // No config at all: the default `Unix` variant materializes WITH its `kind` tag inline
    // and its templated `socket` default resolved.
    let config = manager("");

    let transport: Transport = config.get_config::<Transport>("app.transport").unwrap();

    assert_eq!(
        transport,
        Transport::Unix {
            socket: PathBuf::from("/base/runtime/d.sock"),
        }
    );
}

#[test]
fn internally_tagged_selected_variant_fills_its_fields() {
    // `kind = "tcp"` selects Tcp; the omitted `port`/`addr` fall back to their defaults,
    // filled flat alongside the tag field.
    let config = manager("[app.transport]\nkind = \"tcp\"\n");

    let transport: Transport = config.get_config::<Transport>("app.transport").unwrap();

    assert_eq!(
        transport,
        Transport::Tcp {
            addr: "0.0.0.0".to_string(),
            port: 2116,
        }
    );
}

// Adjacently-tagged: the variant's fields live under a `content` key.
#[config]
#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "t", content = "c", rename_all = "snake_case")]
enum Adj {
    Unit,
    #[default]
    Payload {
        #[default = "7"]
        n: u16,
    },
}

#[test]
fn adjacently_tagged_default_variant_synthesizes_tag_and_content() {
    let config = manager("");

    let adj: Adj = config.get_config::<Adj>("adj").unwrap();

    assert_eq!(adj, Adj::Payload { n: 7 });
}

// The reported bug: an internally-tagged enum whose default `addr` references a *sibling*
// field (`port`) that is itself only a default. The reference must resolve from no config.
#[config(path = "app.transport")]
#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum TransportKind {
    #[default]
    Tcp {
        #[default = "0.0.0.0:${app.transport.port}"]
        addr: String,
        #[default = "2116"]
        port: u16,
    },
    Unix {
        #[default = "${@runtime}/d.sock"]
        socket: PathBuf,
    },
}

#[test]
fn default_resolves_sibling_default_via_seeding() {
    // No config file at all. `addr`'s default references `${app.transport.port}`, whose only
    // value is the `port` default `2116`. Seeding the type into the tree makes it resolve.
    let config = seeded_manager("");

    let transport: TransportKind = config.get_config::<TransportKind>("app.transport").unwrap();

    assert_eq!(
        transport,
        TransportKind::Tcp {
            addr: "0.0.0.0:2116".to_string(),
            port: 2116,
        }
    );
}

#[test]
fn default_resolves_sibling_default_without_auto_discover() {
    // Even without auto-discovery, `get_config` places the type's own filled subtree into the
    // resolution root, so a self/sibling reference still resolves.
    let config = manager("");

    let transport: TransportKind = config.get_config::<TransportKind>("app.transport").unwrap();

    assert_eq!(
        transport,
        TransportKind::Tcp {
            addr: "0.0.0.0:2116".to_string(),
            port: 2116,
        }
    );
}

// Cross-*type* reference: `Downstream`'s default points at a path owned by another config
// type (`Upstream`), whose value there is itself only a default — resolvable only because
// auto-discovery seeds every registered type.
#[config(path = "net.upstream")]
#[derive(Debug, Deserialize, PartialEq)]
struct Upstream {
    #[default = "9000"]
    port: u16,
}

#[config(path = "net.downstream")]
#[derive(Debug, Deserialize, PartialEq)]
struct Downstream {
    #[default = "http://localhost:${net.upstream.port}"]
    url: String,
}

#[test]
fn default_resolves_cross_type_reference_via_seeding() {
    // No config at all; `Downstream.url` references `Upstream`'s defaulted `port`.
    let config = seeded_manager("");

    let downstream: Downstream = config.get_config::<Downstream>("net.downstream").unwrap();

    assert_eq!(downstream.url, "http://localhost:9000");
}

// A cfg-gated default variant: `#[cfg_attr(unix, default)]` marks the platform-specific
// variant as the default (paired with `#[cfg(unix)]` on the variant). This test only asserts
// on platforms where the variant exists.
#[config(path = "cfg.transport")]
#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CfgTransport {
    Tcp {
        #[default = "2116"]
        port: u16,
    },
    #[cfg(unix)]
    #[cfg_attr(unix, default)]
    Unix {
        #[default = "/run/d.sock"]
        socket: PathBuf,
    },
}

#[cfg(unix)]
#[test]
fn cfg_attr_default_variant_selected_on_its_platform() {
    let config = seeded_manager("");

    let transport: CfgTransport = config.get_config::<CfgTransport>("cfg.transport").unwrap();

    assert_eq!(
        transport,
        CfgTransport::Unix {
            socket: PathBuf::from("/run/d.sock"),
        }
    );
}

/// A config type declaring condition facts, registered against the auto-discovered
/// binding path, to prove the staged-value extraction path works end to end.
#[config(path = "flags")]
#[derive(Debug, Clone, Deserialize)]
struct FlagCfg {
    #[default = "false"]
    enabled: bool,
}

impl ConditionFacts for FlagCfg {
    fn condition_facts() -> Vec<ConfigFactDescriptor> {
        vec![ConfigFactDescriptor {
            id: ConfigFactId::new("FlagCfg", "flags", "enabled"),
            kind: ConditionScalarKind::Bool,
            source: DescriptorSource::UNKNOWN,
        }]
    }

    fn condition_scalars(&self) -> Vec<(ConfigFactId, ConditionScalar)> {
        vec![(
            ConfigFactId::new("FlagCfg", "flags", "enabled"),
            ConditionScalar::Bool(self.enabled),
        )]
    }
}

#[test]
fn registered_condition_facts_extract_from_staged_values() {
    let manager = seeded_manager("[flags]\nenabled = true\n");
    let source = ConditionFactSource::of::<FlagCfg>("flags");

    let descriptors = (source.facts.descriptors)();

    assert_eq!(descriptors.len(), 1);
    assert_eq!(descriptors[0].id.property_path, "enabled");

    // Unchanged bindings stage an `Arc` of the value; changed ones stage the plain
    // value — both shapes must extract.
    let value: FlagCfg = manager.get_config::<FlagCfg>("flags").unwrap();
    let shared = (source.facts.scalars)(&Arc::new(value.clone()));
    let plain = (source.facts.scalars)(&value);

    assert_eq!(
        shared,
        vec![(
            ConfigFactId::new("FlagCfg", "flags", "enabled"),
            ConditionScalar::Bool(true)
        )]
    );
    assert_eq!(plain, shared);
}
