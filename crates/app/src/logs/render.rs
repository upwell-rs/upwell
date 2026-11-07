//! Reading a scope's loggable parameters.

use std::fmt::{self, Debug, Display, Formatter};

use upwell_core::{LogParams, Scope};

use super::LogField;

/// The loggable parameters of `scope`, rendered once.
pub(super) fn collect<S: Scope>(scope: &S) -> Vec<LogField> {
    let mut collector = Collector(Vec::new());

    scope.record_log_params(&mut collector);

    collector.0
}

/// Collects each reported parameter as a rendered field.
struct Collector(Vec<LogField>);

impl LogParams for Collector {
    fn param(&mut self, name: &'static str, value: &dyn Debug) {
        self.0.push(LogField::new(name, format!("{value:?}")));
    }
}

/// Renders a scope's loggable parameters as `name=value` pairs, only when a layer asks the
/// span for the field, so a filtered-out span renders nothing.
pub(super) struct Rendered<'a, S: Scope>(pub(super) &'a S);

impl<S: Scope> Display for Rendered<'_, S> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let mut writer = Writer {
            formatter,
            first: true,
            result: Ok(()),
        };

        self.0.record_log_params(&mut writer);

        writer.result
    }
}

/// Writes each reported parameter into a formatter, keeping the first error.
struct Writer<'a, 'b> {
    formatter: &'a mut Formatter<'b>,
    first: bool,
    result: fmt::Result,
}

impl LogParams for Writer<'_, '_> {
    fn param(&mut self, name: &'static str, value: &dyn Debug) {
        if self.result.is_err() {
            return;
        }

        let separator = if self.first { "" } else { " " };

        self.first = false;
        self.result = write!(self.formatter, "{separator}{name}={value:?}");
    }
}
