//! The shared, protocol-generic topic bus, injectable from any axum handler.
//!
//! [`TopicBus<P>`] owns the [`SubscriptionRegistry`] used by a pub/sub endpoint, generic over the
//! protocol `P`. It is a framework-provided singleton, so ordinary HTTP controllers and message
//! controllers publish through the same registry and reach the same live subscriptions. STOMP's
//! instantiation is [`StompTopicBus`](crate::ws::stomp::StompTopicBus) (`TopicBus<Stomp>`),
//! registered as a DI singleton in [`crate::ws::stomp`]; a new protocol registers its own
//! `TopicBus<ThatProtocol>` descriptor the same way.

use std::future::Future;
use std::sync::Arc;

use upwell_core::{Singleton, TypeDescriptor};
use upwell_di::{
    BoxedComponent, Component, ComponentConstructionContext, ComponentDescriptor,
    ComponentFactoryDescriptor, DependencyDescriptor, Injectable,
};
use upwell_hooks::no_hooks;
use upwell_transport::CodecError;

use crate::messaging::Topic;
use crate::ws::PubSubProtocol;

use super::registry::SubscriptionRegistry;

/// The fan-out concurrency used by the ergonomic [`publish`](TopicBus::publish) /
/// [`Publisher::publish`](super::Publisher::publish) — a sensible default for typical subscriber
/// counts. Use `publish_to::<N>` to pick an explicit fan-out.
pub const DEFAULT_PUBLISH_FANOUT: usize = 16;

/// A registry-backed publish handle shared by every endpoint and every axum request for protocol
/// `P`. Cheap to clone (an `Arc` onto the shared registry).
pub struct TopicBus<P: PubSubProtocol> {
    registry: Arc<SubscriptionRegistry<P::OutFrame>>,
}

impl<P: PubSubProtocol> Clone for TopicBus<P> {
    fn clone(&self) -> Self {
        Self {
            registry: Arc::clone(&self.registry),
        }
    }
}

impl<P: PubSubProtocol> TopicBus<P> {
    /// Creates an empty topic bus.
    pub fn new() -> Self {
        Self {
            registry: Arc::new(SubscriptionRegistry::new()),
        }
    }

    /// The shared subscription registry backing this bus.
    pub fn registry(&self) -> &Arc<SubscriptionRegistry<P::OutFrame>> {
        &self.registry
    }

    /// Fire-and-forget publish of a typed topic value to every current subscriber: encodes the
    /// payload and fans it out synchronously. Returns only the encode error; the broadcast itself
    /// never blocks or fails (the registry delivers into each subscriber's buffer without awaiting,
    /// so a slow/dead subscriber is the registry's concern, not the publisher's) — hence no `async`.
    ///
    /// Use [`publish`](Self::publish) when you need to *know* the message reached every live
    /// subscriber's buffer (backpressure) rather than being dropped for a full one.
    pub fn emit<T>(&self, topic: T) -> Result<(), CodecError>
    where
        T: Topic<Protocol = P>,
    {
        let body = topic.encode()?;
        let destination = topic.destination();

        self.publish_raw(destination.as_ref(), &body);

        Ok(())
    }

    /// Awaited publish with **backpressure**, at the [default fan-out](DEFAULT_PUBLISH_FANOUT).
    /// Unlike [`emit`](Self::emit), when this resolves the message is committed to every still-live
    /// subscriber's buffer — a slow consumer makes this wait instead of losing the message. Reach
    /// for [`publish_to`](Self::publish_to) to tune the fan-out concurrency.
    pub async fn publish<T>(&self, topic: T) -> Result<(), CodecError>
    where
        T: Topic<Protocol = P>,
    {
        self.publish_to::<DEFAULT_PUBLISH_FANOUT, T>(topic).await
    }

    /// Awaited publish with **backpressure**, fanning out to up to `N` subscribers concurrently.
    /// Like [`publish`](Self::publish) but with an explicit fan-out concurrency (`N = 1` is
    /// sequential); pick it for the subscriber count you expect.
    pub async fn publish_to<const N: usize, T>(&self, topic: T) -> Result<(), CodecError>
    where
        T: Topic<Protocol = P>,
    {
        let body = topic.encode()?;
        let destination = topic.destination();

        self.registry
            .deliver_frames::<N>(destination.as_ref(), |sub_id, message_id| {
                P::frame_message(message_id, destination.as_ref(), sub_id, &body, &[])
            })
            .await;

        Ok(())
    }

    /// Publishes an already-encoded body to `destination`.
    pub fn publish_raw(&self, destination: &str, body: &P::Body) {
        self.registry
            .publish_frames(destination, |sub_id, message_id| {
                P::frame_message(message_id, destination, sub_id, body, &[])
            });
    }
}

impl<P: PubSubProtocol> Default for TopicBus<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: PubSubProtocol> Component for TopicBus<P> {
    type Handle = Self;

    const ID: &'static str = "upwell:axum:topic-bus";
    const NAME: &'static str = "TopicBus";

    fn into_handle(self) -> Self::Handle {
        self
    }
}

impl<P: PubSubProtocol> Injectable for TopicBus<P> {
    type Target = Self;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

fn topic_bus_dependencies() -> Vec<DependencyDescriptor> {
    Vec::new()
}

fn construct_topic_bus<P: PubSubProtocol>(
    _: &mut ComponentConstructionContext,
) -> std::pin::Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async {
        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<TopicBus<P>>(std::any::type_name::<TopicBus<P>>()),
            value: Box::new(Injectable::into_stored(TopicBus::<P>::new())),
        })
    })
}

fn topic_bus_factories<P: PubSubProtocol>() -> &'static [ComponentFactoryDescriptor] {
    &[ComponentFactoryDescriptor {
        id: "static",
        construct: construct_topic_bus::<P>,
        dependencies: topic_bus_dependencies,
        default: true,
    }]
}

/// Builds the DI descriptor for one protocol's shared topic bus.
pub fn topic_bus_descriptor<P: PubSubProtocol>() -> ComponentDescriptor {
    let name = std::any::type_name::<TopicBus<P>>();
    let id = Box::leak(format!("upwell:axum:topic-bus:{name}").into_boxed_str());

    ComponentDescriptor {
        id,
        name,
        ty: TypeDescriptor::of::<TopicBus<P>>(name),
        scope: &Singleton,
        condition: None,
        factories: topic_bus_factories::<P>,
        hooks: no_hooks,
        generation_snapshot: ComponentDescriptor::typed_snapshot::<TopicBus<P>>(),
    }
}

/// Registers one protocol's shared topic bus in the application DI graph.
pub fn register_topic_bus<P: PubSubProtocol>(registry: &mut upwell_app::AppRegistry) {
    registry.components.push(topic_bus_descriptor::<P>());
}

#[cfg(feature = "di-check")]
impl<P: PubSubProtocol> upwell_di::Provide<TopicBus<P>> for upwell_di::Wiring {}
