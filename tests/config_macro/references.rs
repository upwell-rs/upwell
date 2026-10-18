//! Defaults that reference sibling defaults, other config types' paths, and cfg-gated
//! default variants.

use super::*;

/// The reported bug: an internally-tagged enum whose default `addr` references a *sibling*
/// field (`port`) that is itself only a default. The reference must resolve from no config.
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

/// Cross-*type* reference: `Downstream`'s default points at a path owned by another config
/// type (`Upstream`), whose value there is itself only a default — resolvable only because
/// auto-discovery seeds every registered type.
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

/// A cfg-gated default variant: `#[cfg_attr(unix, default)]` marks the platform-specific
/// variant as the default (paired with `#[cfg(unix)]` on the variant). This test only asserts
/// on platforms where the variant exists.
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
