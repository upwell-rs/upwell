//! The sink of the instance the calling code runs in, for [`ScopeLogs`].

use std::sync::Arc;

use upwell_core::DependencyDescriptor;
use upwell_di::{ComponentConstructionContext, FromContainer};

use super::{ScopeLogSink, ScopeLogStream, route};
use crate::builtins::RuntimeReloader;

/// The log of the scope instance a component or run closure runs in, injectable as a
/// parameter of `ScopeBuilder::run` work.
///
/// It follows the invocation context, as the layer's routing does: inside an invocation it is
/// the log of the instance the context runs in, or of an instance opening under it, and work
/// carried with Upwell's propagation primitives sees the log of the invocation it was captured
/// in. Work captured outside every invocation sees no log. Code that runs outside the
/// invocation machinery entirely sees the log of the `upwell.scope` span it runs in. A
/// component sees the log of the code that resolves it, which for a component built while its
/// instance opens is that instance's; routing a lazily built component to its owning scope is
/// a follow-up (#470). Only a capture the current default subscriber's
/// [`ScopeLogLayer`](super::ScopeLogLayer) took is reported, so code running under another
/// subscriber, whose events never reach it, sees no log. [`current`](Self::current) returns a
/// snapshot of the sink at that moment: a handle moved into other work keeps its original sink.
/// In an instance that does not capture, [`subscribe`](Self::subscribe) returns a stream that
/// has already ended.
#[derive(Clone)]
pub struct ScopeLogs {
    sink: Option<Arc<ScopeLogSink>>,
}

impl ScopeLogs {
    /// The log of the instance the calling code runs in.
    pub fn current() -> Self {
        let (span_sink, admits) = located();
        let span_sink = span_sink.filter(|sink| admits(sink));

        Self {
            sink: route::route(span_sink, admits),
        }
    }

    /// Whether events made where this log was taken are captured: by the instance itself, or
    /// by the enclosing capturing instance a non-capturing one passes its events to. Like
    /// capture, it needs a [`ScopeLogLayer`](super::ScopeLogLayer) whose filter admits the
    /// instance's routing span, so an explicit `upwell::scope=off` reports no capture.
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

/// The sink of the innermost `upwell.scope` span the current default subscriber's layer saw
/// entered on this thread, and which sinks the events made here reach: those that layer took.
/// Locks no span and works with every subscriber the layer supports.
#[cfg(feature = "tracing-subscriber")]
fn located() -> (Option<Arc<ScopeLogSink>>, impl Fn(&ScopeLogSink) -> bool) {
    let reach = super::reach::Reach::current();
    let span_sink = super::sink::any_open()
        .then(|| super::entered::innermost(reach))
        .flatten();

    (span_sink, move |sink: &ScopeLogSink| reach.admits(sink))
}

/// Without a layer no span holds a sink, and no sink is ever taken.
#[cfg(not(feature = "tracing-subscriber"))]
fn located() -> (Option<Arc<ScopeLogSink>>, impl Fn(&ScopeLogSink) -> bool) {
    (None, ScopeLogSink::is_attached)
}
