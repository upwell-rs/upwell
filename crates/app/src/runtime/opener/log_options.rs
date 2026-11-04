//! Log capture options of a [`ScopeBuilder`].

use tracing::level_filters::LevelFilter;
use upwell_core::Scope;

use super::ScopeBuilder;

impl<S: Scope, P: Scope> ScopeBuilder<'_, S, P> {
    /// Captures the events made inside the instance, for [`OpenScope::logs`](super::OpenScope::logs)
    /// or [`ScopeRun::logs`](super::ScopeRun::logs). Capture is off by default, so an instance
    /// nobody asked about costs nothing per event.
    ///
    /// It applies to an instance opened with `open`, `open_default`, `open_on_demand`,
    /// `open_with_bound_seed` and `run`, and needs a
    /// [`ScopeLogLayer`](crate::logs::ScopeLogLayer) in the subscriber.
    pub fn capture_logs(mut self) -> Self {
        self.logs.capture = true;

        self
    }

    /// Captures the instance's events at `level` and more severe ones, instead of at what the
    /// subscriber's filter admits. A stricter level narrows capture; a more verbose one reaches
    /// only as far as the layer's filter lets events through.
    pub fn capture_level(mut self, level: impl Into<LevelFilter>) -> Self {
        self.logs.capture = true;
        self.logs.level = Some(level.into());

        self
    }

    /// Holds up to `records` in each subscriber's buffer before it drops records and reports
    /// them as lagged. At least one.
    pub fn log_buffer(mut self, records: usize) -> Self {
        self.logs.buffer = records;

        self
    }
}
