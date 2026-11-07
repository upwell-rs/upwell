//! The invocation target the job scheduler establishes contexts for.

use std::sync::Arc;

use upwell_di::context::InvocationTarget;

/// One run of a scheduled job. Built once when the job is registered and shared by each run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobTarget {
    /// The job's name.
    pub name: Arc<str>,
}

impl InvocationTarget for JobTarget {}
