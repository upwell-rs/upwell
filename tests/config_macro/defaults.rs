//! Field and enum-variant defaults, serde renames, and the directory namespace.

use super::*;

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
