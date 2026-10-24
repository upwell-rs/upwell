use thiserror::Error;

use crate::error::TemplateError;
use crate::value::ConfigValue;

use super::source::merge_into;

/// A failure while recording one [`ConfigOverlay`] assignment.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigOverlayError {
    /// The property path is empty or has a segment outside `[A-Za-z0-9_-]+`.
    #[error("invalid config overlay path '{path}': expected dot-separated [A-Za-z0-9_-] segments")]
    InvalidPath {
        /// The rejected property path.
        path: String,
    },
    /// The value is a structurally invalid template (an unterminated `${`).
    #[error("invalid config overlay value at '{path}'")]
    InvalidValue {
        /// The property path the value was assigned to.
        path: String,
        /// The template failure, which never carries the value itself.
        #[source]
        source: TemplateError,
    },
    /// A numeric literal does not fit an integer or a finite float.
    #[error("config overlay number at '{path}' is out of range")]
    OutOfRange {
        /// The property path the value was assigned to.
        path: String,
    },
}

/// Property assignments merged above every config source, so a proposed change can be
/// staged without editing a source file.
///
/// Each assignment replaces the node at its dotted path; later assignments win. Values are
/// scalar literals: `true`/`false`, integers (`i128`), finite floats, `"quoted strings"`, and
/// otherwise the bare text as a string. A number outside those ranges is rejected rather than
/// converted. String values keep `${…}` placeholders, exactly as a source file
/// would. Overlays are ordinary config sources: bound defaults still fill missing leaves
/// underneath them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConfigOverlay {
    root: Option<ConfigValue>,
    paths: Vec<String>,
}

impl ConfigOverlay {
    /// Assigns the scalar `literal` at the dotted property `path`.
    pub fn set(&mut self, path: &str, literal: &str) -> Result<(), ConfigOverlayError> {
        let valid = !path.is_empty()
            && path.split('.').all(|segment| {
                !segment.is_empty()
                    && segment
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            });

        if !valid {
            return Err(ConfigOverlayError::InvalidPath {
                path: path.to_string(),
            });
        }

        let value = scalar(path, literal)?;
        let nested = path.rsplit('.').fold(value, |value, segment| {
            ConfigValue::Table(vec![(segment.to_string(), value)])
        });

        match &mut self.root {
            Some(root) => merge_into(root, nested),
            None => self.root = Some(nested),
        }

        self.paths.push(path.to_string());

        Ok(())
    }

    /// Whether no assignment was recorded.
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    /// The assigned property paths, in assignment order. Values are never exposed.
    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    /// Merges every assignment into `root`.
    pub(crate) fn apply(&self, root: &mut ConfigValue) {
        if let Some(overlay) = &self.root {
            merge_into(root, overlay.clone());
        }
    }
}

/// Parses one scalar literal assigned at `path`. Floats must be numeric text, so words such
/// as `inf` or `nan` stay strings; integer text never degrades to a float.
fn scalar(path: &str, literal: &str) -> Result<ConfigValue, ConfigOverlayError> {
    let digits = literal.strip_prefix(['-', '+']).unwrap_or(literal);
    let integral = !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit());
    let numeric = literal
        .bytes()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b'+' | b'e' | b'E'));
    let out_of_range = || ConfigOverlayError::OutOfRange {
        path: path.to_string(),
    };

    if let Ok(boolean) = literal.parse::<bool>() {
        return Ok(ConfigValue::Bool(boolean));
    }

    if integral {
        return literal
            .parse::<i128>()
            .map(ConfigValue::Int)
            .map_err(|_| out_of_range());
    }

    if numeric && let Ok(float) = literal.parse::<f64>() {
        return float
            .is_finite()
            .then_some(ConfigValue::Float(float))
            .ok_or_else(out_of_range);
    }

    let text = literal
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
        .unwrap_or(literal);

    ConfigValue::parsed_str(text).map_err(|source| ConfigOverlayError::InvalidValue {
        path: path.to_string(),
        source,
    })
}

#[cfg(test)]
mod tests;
