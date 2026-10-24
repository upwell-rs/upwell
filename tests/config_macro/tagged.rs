//! Internally and adjacently tagged enums whose default variant synthesizes the tag.

use super::*;

/// An internally-tagged enum (`tag = "kind"`) with a default struct variant — the real-world
/// shape that previously failed with "missing field `kind`".
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

/// Adjacently-tagged: the variant's fields live under a `content` key.
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
