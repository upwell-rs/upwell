//! [`Publisher<T>`]: the typed, injected publish surface.
//!
//! A handler takes `Inject<Publisher<ChatTopics>>` and calls `publish(ChatTopics::Room(msg))`. The
//! destination and body come from the [`Topic`] value itself, so a wrong payload type for a topic
//! is a compile error, not a runtime mismatch. `Publisher<T>` is not a seeded component — it is a
//! [`FromContainer`] that resolves the shared [`TopicBus`](super::TopicBus) for the topic set's
//! protocol (`T::Protocol`), so it works from both message handlers and ordinary HTTP handlers
//! without a per-`T` descriptor.

use std::marker::PhantomData;

use upwell_di::{ComponentConstructionContext, DependencyDescriptor, FromContainer};
use upwell_transport::CodecError;

use crate::messaging::Topic;
use crate::ws::PubSubProtocol;

use super::topic_bus::TopicBus;

/// Publishes typed [`Topic`] values to the bus. Generic over a topic-set `T` (an enum via
/// `#[topics]`), so `publish` only accepts that set's variants and routes to `T::Protocol`'s bus.
pub struct Publisher<T: Topic>
where
    T::Protocol: PubSubProtocol,
{
    bus: TopicBus<T::Protocol>,
    _marker: PhantomData<fn(T)>,
}

impl<T> Publisher<T>
where
    T: Topic,
    T::Protocol: PubSubProtocol,
{
    /// Fire-and-forget: fans a topic value out to its subscribers — reads its destination and
    /// serializes its payload (both from the [`Topic`] impl), then publishes through the bus.
    /// Synchronous (the registry never awaits on fan-out), so a REST handler can emit without being
    /// `async` for that alone; a full subscriber buffer drops the message and only a payload-encoding
    /// failure is returned. Reach for [`publish`](Self::publish) when a drop is not acceptable.
    pub fn emit(&self, topic: T) -> Result<(), CodecError> {
        self.bus.emit(topic)
    }

    /// Awaited fan-out with **backpressure**, at the default fan-out concurrency. Unlike
    /// [`emit`](Self::emit), when this resolves the message is committed to every live subscriber's
    /// buffer rather than dropped for a full one — the confirmation you want when the publish must
    /// not be lost. Reach for [`publish_to`](Self::publish_to) to tune the fan-out.
    pub async fn publish(&self, topic: T) -> Result<(), CodecError> {
        self.bus.publish::<T>(topic).await
    }

    /// Awaited fan-out with **backpressure**, to up to `N` subscribers concurrently. Like
    /// [`publish`](Self::publish) but with an explicit fan-out concurrency (`N = 1` is sequential).
    pub async fn publish_to<const N: usize>(&self, topic: T) -> Result<(), CodecError> {
        self.bus.publish_to::<N, T>(topic).await
    }
}

impl<T> FromContainer for Publisher<T>
where
    T: Topic + Send + Sync + 'static,
    T::Protocol: PubSubProtocol,
    TopicBus<T::Protocol>: FromContainer,
{
    fn dependency() -> DependencyDescriptor {
        // A `Publisher<T>` needs only the shared bus; report that edge so DI validation and ordering
        // see the real dependency (not the phantom `T`).
        <TopicBus<T::Protocol> as FromContainer>::dependency()
    }

    async fn from_container(cx: &ComponentConstructionContext) -> upwell_di::Result<Self> {
        let bus = <TopicBus<T::Protocol> as FromContainer>::from_container(cx).await?;

        Ok(Self {
            bus,
            _marker: PhantomData,
        })
    }
}

/// Under `di-check`, a `Publisher<T>` resolves from its protocol's shared bus, so it is provided
/// exactly when that bus is — the guarantee is conditioned on `Wiring: Provided<TopicBus<T::Protocol>>`
/// (STOMP marks its bus provided). A custom protocol whose bus is never registered therefore fails
/// the compile-time check instead of passing it and blowing up at injection time.
#[cfg(feature = "di-check")]
impl<T> upwell_di::Provided<Publisher<T>> for upwell_di::Wiring
where
    T: Topic,
    T::Protocol: PubSubProtocol,
    upwell_di::Wiring: upwell_di::Provided<TopicBus<T::Protocol>>,
{
}
