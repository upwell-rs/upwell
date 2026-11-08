//! Which sinks the events of the calling code reach under the current default subscriber, for
//! [`ScopeLogs`](super::ScopeLogs).

use tracing::subscriber::NoSubscriber;

use super::sink::LayerId;
use super::{ScopeLogLayer, ScopeLogSink};

/// The sinks the events made here reach, by the layer that took them, so the log is reported
/// only where the current default subscriber delivers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reach {
    /// The sinks the current subscriber's scope log layer took.
    Layer(LayerId),
    /// Every sink a layer took, read from the innermost span any layer recorded entered on
    /// this thread: the code runs inside a `tracing` dispatch, which hides the subscriber
    /// dispatching.
    Any,
    /// No sink: the current subscriber exposes no scope log layer.
    Nothing,
}

impl Reach {
    /// The reach of the current default subscriber.
    ///
    /// Finds the subscriber's [`ScopeLogLayer`] by downcasting, which `Layered`, `Filtered`,
    /// boxed and `Vec` layers forward; a `reload` layer does not, so a scope log layer wrapped
    /// in one, rather than behind a reloadable filter, reaches no sink here. With several scope
    /// log layers in one subscriber, an unsupported setup in which each delivers, the first one
    /// the downcast finds decides. Inside a dispatch, as when a formatter renders a value that
    /// asks for the log, `tracing` reports no subscriber at all, so the reach falls back to
    /// every sink a layer took, as before layers had identities; an explicit `Dispatch::none`
    /// default is indistinguishable from that and falls back too.
    pub(super) fn current() -> Self {
        tracing::dispatcher::get_default(|dispatch| {
            match dispatch.downcast_ref::<ScopeLogLayer>() {
                Some(layer) => Self::Layer(layer.id()),
                None if dispatch.is::<NoSubscriber>() => Self::Any,
                None => Self::Nothing,
            }
        })
    }

    /// Whether the events made here reach `sink`.
    pub(super) fn admits(self, sink: &ScopeLogSink) -> bool {
        match self {
            Self::Layer(layer) => sink.is_attached_to(layer),
            Self::Any => sink.is_attached(),
            Self::Nothing => false,
        }
    }
}
