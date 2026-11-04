//! The sink of the instance a task runs in, for [`ScopeLogs`].

use std::future::Future;
use std::sync::Arc;

use upwell_core::DependencyDescriptor;
use upwell_di::{ComponentConstructionContext, FromContainer};

use super::{ScopeLogSink, ScopeLogStream};
use crate::builtins::RuntimeReloader;

tokio::task_local! {
    static CURRENT: Option<Arc<ScopeLogSink>>;
}

/// Runs `future` with `sink` as the sink of the instance it runs in, or with none for an
/// instance that does not capture, so it never sees an enclosing instance's.
pub(crate) fn with_current_sink<F: Future>(
    sink: Option<Arc<ScopeLogSink>>,
    future: F,
) -> impl Future<Output = F::Output> {
    CURRENT.scope(sink, future)
}

/// The log of the scope instance a component or run closure runs in, injectable as a
/// parameter of `ScopeBuilder::run` work.
///
/// It is task-local access, in the sense of `tracing`'s current span: it reflects the capturing
/// instance the *current task* runs in, so it is meant for `run` work and entries, not as a
/// property of a component. Work spawned without Upwell's propagation primitives does not see
/// it. A component built lazily, or as an ancestor while a child instance opens, sees the sink
/// of the task that resolves it, not of its own scope; routing construction to the owning
/// scope's sink is a follow-up (#470). In an instance that does not capture,
/// [`subscribe`](Self::subscribe) returns a stream that has already ended.
#[derive(Clone)]
pub struct ScopeLogs {
    sink: Option<Arc<ScopeLogSink>>,
}

impl ScopeLogs {
    /// The log of the instance the calling task runs in.
    pub fn current() -> Self {
        Self {
            sink: CURRENT.try_with(Clone::clone).ok().flatten(),
        }
    }

    /// Whether the instance captures logs.
    pub fn is_capturing(&self) -> bool {
        self.sink.is_some()
    }

    /// A stream of the records the instance captures from now on.
    pub fn subscribe(&self) -> ScopeLogStream {
        match &self.sink {
            Some(sink) => sink.subscribe(),
            None => ScopeLogStream::ended(),
        }
    }
}

impl FromContainer for ScopeLogs {
    fn dependency() -> DependencyDescriptor {
        <RuntimeReloader as FromContainer>::dependency()
    }

    async fn from_container(_cx: &ComponentConstructionContext) -> upwell_di::Result<Self> {
        Ok(Self::current())
    }
}

impl upwell_di::Provided<ScopeLogs> for upwell_di::Wiring {}
