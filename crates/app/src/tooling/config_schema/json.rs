//! JSON Schema (2020-12) rendering of config shapes.
//!
//! Standard keywords (`type`, `properties`, `required`, `description`, `default`, `oneOf`) drive
//! editor completion and validation directly; `x-upwell-*` keywords carry what JSON Schema cannot
//! express — Rust types, declaration sources, and template support — for an IDE plugin.

use serde_json::{Map, Value, json};
use upwell_config::EnumTag;
use upwell_config::schema::{
    ConfigSchema, ConfigShape, FieldSchema, ValueSchema, VariantPayload, VariantSchema,
};
use upwell_core::DescriptorSource;

/// The `$defs` key of the template string schema every non-string scalar also accepts.
pub(super) const TEMPLATE_DEF: &str = "upwell-template";

/// Bounds nested `#[config]` expansion so a recursive config type terminates.
const MAX_DEPTH: usize = 32;

/// The `$defs` shared by every rendered root schema.
pub(super) fn definitions() -> Value {
    json!({
        TEMPLATE_DEF: {
            "type": "string",
            "pattern": "\\$\\{[^}]*\\}",
            "description": "An Upwell template: `${key}` or `${key:default}`, resolved when the config binds.",
        }
    })
}

/// The JSON Schema of one value shape.
pub(super) fn value(schema: &ValueSchema) -> Value {
    value_at(schema, 0)
}

fn value_at(schema: &ValueSchema, depth: usize) -> Value {
    match schema {
        ValueSchema::Bool => templated(json!({ "type": "boolean" })),
        ValueSchema::Integer { signed: true } => templated(json!({ "type": "integer" })),
        ValueSchema::Integer { signed: false } => {
            templated(json!({ "type": "integer", "minimum": 0 }))
        }
        ValueSchema::Float => templated(json!({ "type": "number" })),
        ValueSchema::Char => json!({ "type": "string", "maxLength": 1, "x-upwell-template": true }),
        ValueSchema::String | ValueSchema::Bytes => {
            json!({ "type": "string", "x-upwell-template": true })
        }
        ValueSchema::Unit => json!({ "type": "null" }),
        ValueSchema::Optional(inner) => {
            json!({ "anyOf": [value_at(inner, depth + 1), { "type": "null" }] })
        }
        ValueSchema::Array(item) => json!({ "type": "array", "items": value_at(item, depth + 1) }),
        ValueSchema::Map(item) => {
            json!({ "type": "object", "additionalProperties": value_at(item, depth + 1) })
        }
        ValueSchema::Tuple(items) => json!({
            "type": "array",
            "prefixItems": items.iter().map(|item| value_at(item, depth + 1)).collect::<Vec<_>>(),
            "minItems": items.len(),
            "maxItems": items.len(),
        }),
        ValueSchema::Config { rust_type, schema } => config_at(rust_type, schema, depth + 1),
        ValueSchema::Struct { name, fields } => {
            let properties = fields
                .iter()
                .map(|field| (field.name.to_string(), value_at(&field.value, depth + 1)))
                .collect::<Map<_, _>>();

            json!({
                "type": "object",
                "title": name,
                "properties": properties,
                "x-upwell-traced": true,
            })
        }
        ValueSchema::Enum { name, variants } => json!({
            "title": name,
            "enum": variants,
            "x-upwell-traced": true,
        }),
        _ => json!({}),
    }
}

/// The JSON Schema of a declared config type.
pub(super) fn config(rust_type: &str, schema: &ConfigSchema) -> Value {
    config_at(rust_type, schema, 0)
}

fn config_at(rust_type: &str, schema: &ConfigSchema, depth: usize) -> Value {
    if depth > MAX_DEPTH {
        return json!({ "x-upwell-rust-type": rust_type });
    }

    let type_path = schema
        .source
        .map(|source| source.rust_path())
        .unwrap_or_else(|| rust_type.to_string());
    let mut rendered = match schema.shape {
        ConfigShape::Undeclared => json!({}),
        ConfigShape::Struct(fields) => object(&type_path, fields, depth),
        ConfigShape::Enum { tagging, variants } => {
            enumeration(&type_path, tagging, variants, depth)
        }
    };
    let object = rendered
        .as_object_mut()
        .expect("config schemas render as objects");

    insert_docs(object, schema.docs);
    object.insert(String::from("x-upwell-rust-type"), json!(rust_type));

    if let Some(source) = schema.source {
        object.insert(
            String::from("x-upwell-source"),
            location(source.location, Some(source.rust_path())),
        );
    }

    rendered
}

fn object(type_path: &str, fields: &[FieldSchema], depth: usize) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();

    for field in fields {
        let mut schema = field_value(type_path, field, depth);

        if field.flatten {
            flatten_into(&mut properties, &mut required, &mut schema);

            continue;
        }

        if field.required {
            required.push(json!(field.name));
        }

        properties.insert(field.name.to_string(), schema);
    }

    json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

/// Merges a flattened field's object properties into its parent's.
fn flatten_into(
    properties: &mut Map<String, Value>,
    required: &mut Vec<Value>,
    schema: &mut Value,
) {
    if let Some(Value::Object(inner)) = schema.get_mut("properties").map(Value::take) {
        properties.extend(inner);
    }

    if let Some(Value::Array(inner)) = schema.get_mut("required").map(Value::take) {
        required.extend(inner);
    }
}

fn field_value(type_path: &str, field: &FieldSchema, depth: usize) -> Value {
    let mut schema = value_at(&(field.value)(), depth + 1);
    let object = schema
        .as_object_mut()
        .expect("value schemas render as objects");

    insert_docs(object, field.docs);

    if let Some(default) = field.default {
        object.insert(String::from("default"), json!(default));
    }

    object.insert(
        String::from("x-upwell-field"),
        json!({
            "ident": field.ident,
            "declared-type": field.declared_type,
            "rust-type": (field.rust_type)(),
            "default-template": field.default,
            "source": location(field.source, Some(format!("{type_path}::{}", field.ident))),
        }),
    );

    schema
}

fn enumeration(
    type_path: &str,
    tagging: EnumTag,
    variants: &[VariantSchema],
    depth: usize,
) -> Value {
    let alternatives = variants
        .iter()
        .map(|variant| variant_schema(type_path, tagging, variant, depth))
        .collect::<Vec<_>>();
    let mut rendered = json!({ "oneOf": alternatives });

    if let Some(default) = variants.iter().find(|variant| variant.default) {
        rendered["x-upwell-default-variant"] = json!(default.name);
    }

    rendered
}

fn variant_schema(
    type_path: &str,
    tagging: EnumTag,
    variant: &VariantSchema,
    depth: usize,
) -> Value {
    let payload = payload(type_path, &variant.payload, depth);
    let tag = json!({ "const": variant.name });
    let mut rendered = match (tagging, &variant.payload) {
        (EnumTag::External, VariantPayload::Unit) => tag,
        (EnumTag::External, _) => json!({
            "type": "object",
            "properties": { variant.name: payload },
            "required": [variant.name],
            "additionalProperties": false,
        }),
        (EnumTag::Internal { tag: key }, _) => {
            let mut object = match payload {
                Value::Object(object) if object.contains_key("properties") => object,
                _ => Map::from_iter([
                    (String::from("type"), json!("object")),
                    (String::from("properties"), json!({})),
                ]),
            };

            object["properties"][key] = tag;
            push_required(&mut object, key);

            Value::Object(object)
        }
        (EnumTag::Adjacent { tag: key, content }, VariantPayload::Unit) => json!({
            "type": "object",
            "properties": { key: tag },
            "required": [key],
            "x-upwell-content": content,
        }),
        (EnumTag::Adjacent { tag: key, content }, _) => json!({
            "type": "object",
            "properties": { key: tag, content: payload },
            "required": [key, content],
        }),
        (EnumTag::Untagged, _) => payload,
    };
    let object = rendered
        .as_object_mut()
        .expect("variant schemas render as objects");

    insert_docs(object, variant.docs);
    object.insert(
        String::from("x-upwell-variant"),
        json!({
            "ident": variant.ident,
            "tag": variant.name,
            "default": variant.default,
            "source": location(variant.source, Some(format!("{type_path}::{}", variant.ident))),
        }),
    );

    rendered
}

fn payload(type_path: &str, payload: &VariantPayload, depth: usize) -> Value {
    match payload {
        VariantPayload::Unit => json!({ "type": "null" }),
        VariantPayload::Struct(fields) => object(type_path, fields, depth),
        VariantPayload::Tuple([single]) => value_at(&single(), depth + 1),
        VariantPayload::Tuple(items) => value_at(
            &ValueSchema::Tuple(items.iter().map(|item| item()).collect()),
            depth,
        ),
    }
}

fn push_required(object: &mut Map<String, Value>, key: &str) {
    let required = object
        .entry(String::from("required"))
        .or_insert_with(|| json!([]));

    if let Value::Array(required) = required {
        required.push(json!(key));
    }
}

/// A non-string scalar that also accepts a template string, as config values may be written
/// `"${PORT}"` and resolve to the field's type when bound.
fn templated(schema: Value) -> Value {
    json!({
        "anyOf": [schema, { "$ref": format!("#/$defs/{TEMPLATE_DEF}") }],
        "x-upwell-template": true,
    })
}

fn insert_docs(object: &mut Map<String, Value>, docs: &str) {
    if !docs.is_empty() {
        object.insert(String::from("description"), json!(docs));
    }
}

/// A declaration position in the tooling document's `SourceLocation` shape.
fn location(source: DescriptorSource, rust_path: Option<String>) -> Value {
    if source.line == 0 {
        return json!({ "rust_path": rust_path });
    }

    json!({
        "file": source.file,
        "line": source.line,
        "column": source.column,
        "rust_path": rust_path,
    })
}
