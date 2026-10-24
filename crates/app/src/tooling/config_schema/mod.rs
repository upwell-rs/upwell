//! Config field schemas for tooling: every bound config type rendered as JSON Schema, on its
//! binding resource and nested by path into one document-level schema of the whole tree.
//!
//! Declared `#[config]` types contribute their fields, docs, defaults, and sources; other bound
//! types contribute the shape traced from their `Deserialize` impl, so runtime-bound and
//! hand-written configs are covered too.

use serde_json::{Map, Value, json};
use upwell_config::ConfigBinding;
use upwell_config::schema::ValueSchema;
use upwell_tooling_schema::{
    CONFIG_BINDING_SCHEMA_FACET, CONFIG_SCHEMA_FACET, CONFIG_SCHEMA_FACET_VERSION, Facet,
};

use super::{Projection, config_binding_id};
use crate::ProtocolDefinition;

mod json;
mod templating;

impl<D: ProtocolDefinition> Projection<'_, D> {
    /// Attaches each binding's schema to its resource and the tree schema to the document.
    pub(super) fn project_config_schema(&mut self, bindings: &[ConfigBinding]) {
        let mut ordered = bindings.iter().collect::<Vec<_>>();
        let mut root = Map::new();

        ordered.sort_by_key(|binding| binding.path.split('.').count());

        for binding in &ordered {
            let schema = binding_schema(binding);
            let id = config_binding_id((binding.ty.type_name)(), &binding.path);

            self.attach_binding_schema(&id, &binding.path, &schema);
            insert_at_path(&mut root, &binding.path, schema);
        }

        let paths = ordered
            .iter()
            .map(|binding| binding.path.clone())
            .collect::<Vec<_>>();

        self.document.facets.insert(
            CONFIG_SCHEMA_FACET.to_string(),
            Facet {
                schema_version: CONFIG_SCHEMA_FACET_VERSION,
                value: json!({
                    "$schema": "https://json-schema.org/draft/2020-12/schema",
                    "title": format!("{} configuration", self.app.name()),
                    "type": "object",
                    "properties": root,
                    "$defs": json::definitions(),
                    "x-upwell-templating": templating::descriptor(&paths),
                }),
            },
        );
    }

    fn attach_binding_schema(&mut self, id: &str, path: &str, schema: &Value) {
        let Some(resource) = self
            .document
            .resources
            .iter_mut()
            .find(|resource| resource.id == id)
        else {
            return;
        };

        resource.facets.insert(
            format!("{id}/{CONFIG_BINDING_SCHEMA_FACET}"),
            Facet {
                schema_version: CONFIG_SCHEMA_FACET_VERSION,
                value: json!({
                    "path": path,
                    "schema": schema,
                    "$defs": json::definitions(),
                }),
            },
        );
    }
}

/// The JSON Schema of the type bound by `binding`.
fn binding_schema(binding: &ConfigBinding) -> Value {
    match (binding.describe)() {
        ValueSchema::Config { rust_type, schema } => json::config(rust_type, &schema),
        traced => {
            let mut schema = json::value(&traced);

            if let Some(object) = schema.as_object_mut() {
                object.insert(
                    String::from("x-upwell-rust-type"),
                    json!((binding.ty.type_name)()),
                );
            }

            schema
        }
    }
}

/// Nests `schema` under the dotted `path` of the root `properties`, keeping any properties a
/// shorter binding path already placed at that node.
fn insert_at_path(root: &mut Map<String, Value>, path: &str, mut schema: Value) {
    let mut segments = path.split('.').filter(|segment| !segment.is_empty());
    let Some(last) = segments.next_back() else {
        return;
    };
    let mut properties = root;

    for segment in segments {
        let node = properties
            .entry(segment.to_string())
            .or_insert_with(|| json!({ "type": "object", "properties": {} }));

        properties = node_properties(node);
    }

    if let Some(Value::Object(existing)) = properties
        .remove(last)
        .and_then(|mut node| node.get_mut("properties").map(Value::take))
    {
        node_properties(&mut schema).extend(existing);
    }

    properties.insert(last.to_string(), schema);
}

/// The `properties` map of an object schema node, created when absent.
fn node_properties(node: &mut Value) -> &mut Map<String, Value> {
    let object = node
        .as_object_mut()
        .expect("config schema nodes are objects");
    let properties = object
        .entry(String::from("properties"))
        .or_insert_with(|| json!({}));

    if !properties.is_object() {
        *properties = json!({});
    }

    properties.as_object_mut().expect("normalized above")
}

#[cfg(test)]
mod tests;
