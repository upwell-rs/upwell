//! A named, rendered value of a log record.

/// One field of a [`LogRecord`](super::LogRecord) or one loggable parameter of a scope
/// instance, with its value rendered to a string.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogField {
    /// The field's name.
    pub name: &'static str,
    /// The field's value as its `Debug` (or, for text, its `str`) rendering.
    pub value: String,
}

impl LogField {
    /// A field called `name` holding `value`.
    pub fn new(name: &'static str, value: String) -> Self {
        Self { name, value }
    }
}
