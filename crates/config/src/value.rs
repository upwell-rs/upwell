use crate::error::TemplateError;
use crate::parse;

/// A format-agnostic configuration value tree.
///
/// Every source format (TOML, YAML, ...) normalizes into this type, so placeholder
/// parsing, resolution, and the typed deserializer are written exactly once. String
/// leaves are pre-parsed into placeholder segments at construction time, which keeps
/// (re-)deserialization allocation-light and side-effect-free — the property a future
/// hot-reload relies on.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigValue {
    Null,
    Bool(bool),
    /// Source integers, widened to `i128` so any target width fits; the deserializer
    /// narrows on demand via the `deserialize_*` method the target type calls.
    Int(i128),
    Float(f64),
    Str(ConfigStr),
    Array(Vec<ConfigValue>),
    /// An ordered string-keyed table. A `Vec` of pairs preserves insertion order and
    /// avoids an `indexmap` dependency; config tables are small, so linear lookup is fine.
    Table(Vec<(String, ConfigValue)>),
}

impl ConfigValue {
    /// Navigates a dotted property path (`a.b.c`), returning the node at that path.
    /// An empty path returns the root. `None` if any segment is missing or traverses
    /// a non-table.
    pub fn get_path(&self, path: &str) -> Option<&ConfigValue> {
        if path.is_empty() {
            return Some(self);
        }

        lookup_path(self, path)
    }

    /// Builds a string leaf by parsing `raw` for `${...}` placeholders — the inverse of
    /// rendering. Used to inject default values into the tree so a templated default such
    /// as `${tcp.ip}:${tcp.port}` resolves through the same pipeline as a file value.
    /// Errors only on a structurally invalid template (an unterminated `${`).
    pub fn parsed_str(raw: &str) -> Result<ConfigValue, TemplateError> {
        Ok(ConfigValue::Str(ConfigStr::parse(raw)?))
    }

    /// A short type label used in mismatch error messages.
    pub(crate) fn type_label(&self) -> &'static str {
        match self {
            ConfigValue::Null => "null",
            ConfigValue::Bool(_) => "bool",
            ConfigValue::Int(_) => "integer",
            ConfigValue::Float(_) => "float",
            ConfigValue::Str(_) => "string",
            ConfigValue::Array(_) => "array",
            ConfigValue::Table(_) => "table",
        }
    }
}

/// A string leaf decomposed into literal and placeholder segments.
///
/// The `kind` is computed once at parse time so the deserializer can branch on
/// "full placeholder" vs "literal" vs "templated" without re-scanning the segments.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigStr {
    pub(crate) segments: Vec<Segment>,
    pub(crate) kind: StrKind,
}

impl ConfigStr {
    /// A string whose placeholders were already rendered to `text`. A templated source keeps
    /// its kind, so it still only satisfies string targets exactly as before rendering.
    pub(crate) fn rendered(text: String, source: StrKind) -> Self {
        let kind = match source {
            StrKind::Templated => StrKind::Templated,
            StrKind::Literal | StrKind::FullPlaceholder => StrKind::Literal,
        };

        Self {
            segments: vec![Segment::Literal(text)],
            kind,
        }
    }

    /// Parses one raw source string into classified segments. Errors only on a
    /// structurally invalid template (an unterminated `${`).
    pub fn parse(raw: &str) -> Result<Self, TemplateError> {
        let (segments, kind) = parse::parse_template(raw)?;

        Ok(Self { segments, kind })
    }

    /// `true` when the entire leaf is a single `${...}`, so the target type may be
    /// deferred to whatever `deserialize_*` method serde calls.
    pub fn is_full_placeholder(&self) -> bool {
        matches!(self.kind, StrKind::FullPlaceholder)
    }

    /// The single placeholder of a full-placeholder leaf.
    pub(crate) fn as_full(&self) -> Option<&Placeholder> {
        match (self.kind, self.segments.first()) {
            (StrKind::FullPlaceholder, Some(Segment::Placeholder(p))) => Some(p),
            _ => None,
        }
    }

    /// The plain literal text when this leaf has no placeholders, used to read an enum's
    /// tag value (e.g. `kind = "unix"`) without resolving anything.
    pub(crate) fn as_literal(&self) -> Option<&str> {
        match (self.kind, self.segments.as_slice()) {
            (StrKind::Literal, [Segment::Literal(text)]) => Some(text),
            (StrKind::Literal, []) => Some(""),
            _ => None,
        }
    }
}

/// One piece of a templated string.
#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    Literal(String),
    Placeholder(Placeholder),
}

/// A resolved-at-deserialize-time reference with an optional inline default.
#[derive(Debug, Clone, PartialEq)]
pub struct Placeholder {
    /// The lookup key — an env var name (`DATABASE_URL`) or a dotted config path
    /// (`app.server.host`).
    pub key: String,
    /// The inline default from `${key:default}`, if present.
    pub default: Option<String>,
}

/// Pre-computed classification of a [`ConfigStr`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StrKind {
    /// No placeholders: `segments` is exactly one `Literal`.
    Literal,
    /// Exactly one segment, a placeholder, with no surrounding text. Eligible for
    /// type-deferred (full) substitution.
    FullPlaceholder,
    /// A mix of literals and placeholders, or several placeholders. Can only ever
    /// produce a `String`.
    Templated,
}

/// Walks a dotted property path (`a.b.c`) through nested tables, returning the node
/// at that path. Returns `None` if any segment is missing or traverses a non-table.
pub(crate) fn lookup_path<'a>(root: &'a ConfigValue, path: &str) -> Option<&'a ConfigValue> {
    let mut current = root;

    for segment in path.split('.') {
        let table = match current {
            ConfigValue::Table(entries) => entries,
            _ => return None,
        };

        let next = table.iter().find(|(key, _)| key == segment)?;

        current = &next.1;
    }

    Some(current)
}
