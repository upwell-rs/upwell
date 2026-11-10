#![cfg(feature = "ws")]

//! Declared access from a WebSocket message's invocation to a value of its connection scope,
//! opened as a WebSocket protocol opens them: the connection on demand, each message for its
//! handler's entry.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use upwell::context::{Current, Scoped};
use upwell::{component, methods};
use upwell_app::{App, EntryDescriptor};
use upwell_axum::Axum;
use upwell_axum::scope::{WebsocketConnection, WebsocketMessage};
use upwell_config::{ConfigManager, Dynamic};
use upwell_core::EntryKey;
use upwell_di::context::{InvocationTarget, TargetRef};

/// Numbers handed out to [`ConnectionIdentity`]s.
static ISSUED: AtomicUsize = AtomicUsize::new(1);

/// A connection-scoped identity numbered per connection.
#[component(scope = WebsocketConnection)]
struct ConnectionIdentity {
    #[default]
    number: usize,
}

#[methods]
impl ConnectionIdentity {
    #[init]
    async fn init() -> Self {
        Self {
            number: ISSUED.fetch_add(1, Ordering::Relaxed),
        }
    }
}

/// A singleton a message handler injects, reading the connection's identity.
#[component]
struct Listener {
    closest: Current<Arc<ConnectionIdentity>>,
    pinned: Scoped<WebsocketConnection, Arc<ConnectionIdentity>>,
}

/// The entry point the test establishes message contexts for.
#[derive(Debug)]
struct MessageTarget;

impl InvocationTarget for MessageTarget {}

fn key() -> EntryKey {
    EntryKey::new("ws/access.listen").expect("a valid entry key")
}

#[tokio::test]
async fn a_message_reads_its_connections_value_through_both_accessors() {
    let app = App::<Axum>::builder("scoped-access-ws")
        .config_source(ConfigManager::<Dynamic>::empty())
        .component::<ConnectionIdentity>()
        .component::<Listener>()
        .entry_descriptor(EntryDescriptor::new::<WebsocketMessage, (Arc<Listener>,)>(
            key(),
        ))
        .deny_unplanned()
        .build()
        .await
        .expect("the app builds");
    let runtime = app.runtime();
    let connection = runtime
        .opener::<WebsocketConnection>()
        .expect("the connection scope is declared")
        .builder(WebsocketConnection)
        .open_on_demand()
        .await
        .expect("the connection opens");
    let messages = runtime
        .opener::<WebsocketMessage>()
        .expect("the message scope is declared");
    let entry = messages.entry(key()).expect("the entry resolves");
    let message = messages
        .builder(WebsocketMessage)
        .under(&connection)
        .open_when_needed(Some(&entry))
        .await
        .expect("the message opens");
    let context = message.establish(messages.context(TargetRef::from_static(&MessageTarget)));

    let (closest, pinned) = context
        .enter(async {
            let listener = message
                .extract::<Arc<Listener>>()
                .await
                .expect("the handler injects the listener");
            let closest = listener.closest.get().await.expect("the closest identity");
            let pinned = listener.pinned.get().await.expect("the pinned identity");

            (closest.number, pinned.number)
        })
        .await;
    let built = connection
        .container()
        .get::<ConnectionIdentity>()
        .expect("the identity was built in the connection");

    assert_eq!(closest, built.number);
    assert_eq!(pinned, built.number);
}
