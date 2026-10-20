#![allow(dead_code)]

use std::collections::BTreeMap;

use serde::Deserialize;

use super::__probe::{Probe, ViaDescribed as _, ViaOpaque as _, ViaSerde as _};
use super::*;

/// A plain serde struct with no `#[config]` declaration.
#[derive(Deserialize)]
struct Plain {
    host: String,
    port: u16,
    tags: Vec<String>,
    limits: BTreeMap<String, f64>,
    nested: Option<Inner>,
    mode: Mode,
}

/// A nested plain serde struct.
#[derive(Deserialize)]
struct Inner {
    enabled: bool,
}

/// A plain serde enum.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Fast,
    Safe { retries: u8 },
}

/// A self-describing field type.
#[derive(Deserialize)]
#[serde(untagged)]
enum Either {
    Number(u32),
    Text(String),
}

/// A recursive plain serde struct.
#[derive(Deserialize)]
struct Recursive {
    next: Option<Box<Recursive>>,
}

/// A type with no serde impl.
struct Opaque;

fn field<'a>(fields: &'a [TracedField], name: &str) -> &'a ValueSchema {
    &fields
        .iter()
        .find(|field| field.name == name)
        .expect("the traced field exists")
        .value
}

fn traced_fields(schema: ValueSchema) -> Vec<TracedField> {
    match schema {
        ValueSchema::Struct { fields, .. } => fields,

        other => panic!("expected a traced struct, got {other:?}"),
    }
}

#[test]
fn tracing_recovers_fields_scalars_and_collections() {
    let fields = traced_fields(trace::<Plain>());

    assert!(matches!(field(&fields, "host"), ValueSchema::String));
    assert!(matches!(
        field(&fields, "port"),
        ValueSchema::Integer { signed: false }
    ));
    assert!(matches!(
        field(&fields, "tags"),
        ValueSchema::Array(item) if matches!(**item, ValueSchema::String)
    ));
    assert!(matches!(
        field(&fields, "limits"),
        ValueSchema::Map(value) if matches!(**value, ValueSchema::Float)
    ));
    assert!(matches!(
        field(&fields, "nested"),
        ValueSchema::Optional(inner)
            if matches!(&**inner, ValueSchema::Struct { name: "Inner", .. })
    ));
    assert!(matches!(
        field(&fields, "mode"),
        ValueSchema::Enum { variants, .. } if variants == &["fast", "safe"]
    ));
}

#[test]
fn tracing_keeps_self_describing_and_recursive_positions_bounded() {
    assert!(matches!(trace::<Either>(), ValueSchema::Any));
    assert!(matches!(
        trace::<Recursive>(),
        ValueSchema::Struct {
            name: "Recursive",
            ..
        }
    ));
}

/// The `&&&` receivers are the autoref ranking under test, not needless borrows.
#[test]
#[allow(clippy::needless_borrow)]
fn the_probe_prefers_declared_then_traced_then_opaque_shapes() {
    assert!(matches!(
        (&&&Probe::<Option<u16>>::NEW).value_schema(),
        ValueSchema::Optional(inner) if matches!(*inner, ValueSchema::Integer { signed: false })
    ));
    assert!(matches!(
        (&&&Probe::<Inner>::NEW).value_schema(),
        ValueSchema::Struct { name: "Inner", .. }
    ));
    assert!(matches!(
        (&&&Probe::<Opaque>::NEW).value_schema(),
        ValueSchema::Any
    ));
}

/// A string-parsed value accepting only one spelling, like an enum read through `FromStr`.
struct JournalMode;

impl<'de> Deserialize<'de> for JournalMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;

        if text == "wal" {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom("unknown journal mode"))
        }
    }
}

/// Fields that reject the first, or every, traced value, ahead of plainly traced ones.
#[derive(Deserialize)]
struct Strict {
    bind: std::net::IpAddr,
    journal: JournalMode,
    either: Either,
    port: u16,
}

#[test]
fn tracing_retries_strict_values_and_skips_fields_that_reject_every_candidate() {
    let fields = traced_fields(trace::<Strict>());

    assert!(matches!(field(&fields, "bind"), ValueSchema::String));
    assert!(matches!(field(&fields, "journal"), ValueSchema::String));
    assert!(matches!(field(&fields, "either"), ValueSchema::Any));
    assert!(matches!(
        field(&fields, "port"),
        ValueSchema::Integer { signed: false }
    ));
}
