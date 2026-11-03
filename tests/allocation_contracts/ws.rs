//! One WebSocket message: its `WebsocketMessage` scope opened under one connection, as a
//! WebSocket protocol opens it, and its handler's injections read from it. No socket or frame
//! codec is involved; they do the same work for every case.

use std::sync::Arc;

use upwell_app::{App, EntryDescriptor, EntryScope, OpenScope, ScopeEntry, ScopeOpener};
use upwell_axum::Axum;
use upwell_axum::scope::{WebsocketConnection, WebsocketMessage};
use upwell_config::{ConfigManager, Dynamic};
use upwell_core::{EntryKey, Scope};

use crate::Clock;
use crate::alloc::Delta;
use crate::roster::{Scoped, UNUSED, per_call, roster, runtime};

/// What a message handler injects.
#[derive(Clone, Copy, PartialEq)]
enum Handler {
    /// Nothing.
    Hello,
    /// `Arc<Scoped<0>>`, a message-scoped component.
    Scoped,
    /// `Arc<Clock>`, a singleton.
    Clock,
}

impl Handler {
    const ALL: [Self; 3] = [Self::Hello, Self::Scoped, Self::Clock];

    fn key(self) -> EntryKey {
        let key = match self {
            Self::Hello => "ws/contract.hello",
            Self::Scoped => "ws/contract.scoped",
            Self::Clock => "ws/contract.clock",
        };

        EntryKey::new(key).expect("contract entry keys are valid")
    }

    fn entry(self) -> EntryDescriptor {
        match self {
            Self::Hello => EntryDescriptor::new::<WebsocketMessage, ()>(self.key()),
            Self::Scoped => EntryDescriptor::new::<WebsocketMessage, (Arc<Scoped<0>>,)>(self.key()),
            Self::Clock => EntryDescriptor::new::<WebsocketMessage, (Arc<Clock>,)>(self.key()),
        }
    }

    /// Whether an app with `count` message-scoped components can serve this handler.
    fn served_with(self, count: usize) -> bool {
        self != Self::Scoped || count > 0
    }

    /// Runs the handler's injections on `scope`, as generated handler code does.
    async fn run(self, scope: EntryScope<WebsocketMessage>) -> bool {
        match self {
            Self::Hello => true,
            Self::Scoped => scope.extract::<Arc<Scoped<0>>>().await.is_ok(),
            Self::Clock => scope.extract::<Arc<Clock>>().await.is_ok(),
        }
    }
}

/// An axum app with `count` message-scoped components and one open WebSocket connection.
struct Messages {
    _app: App<Axum>,
    connection: OpenScope<WebsocketConnection>,
    messages: ScopeOpener<WebsocketMessage>,
    entries: Vec<(Handler, ScopeEntry<WebsocketMessage>)>,
}

impl Messages {
    async fn build(count: usize) -> Self {
        let builder = App::<Axum>::builder("ws-allocation-contracts")
            .config_source(ConfigManager::<Dynamic>::empty())
            .with_component(Clock);
        let builder = roster(WebsocketMessage::KEY, count)
            .iter()
            .fold(builder, |builder, component| {
                builder.component_descriptor(component)
            });
        let served = || {
            Handler::ALL
                .into_iter()
                .filter(move |handler| handler.served_with(count))
        };
        let app = served()
            .fold(builder, |builder, handler| {
                builder.entry_descriptor(handler.entry())
            })
            .build()
            .await
            .expect("the contract app builds");
        let connection = app
            .runtime()
            .opener::<WebsocketConnection>()
            .expect("the connection scope is declared")
            .builder(WebsocketConnection)
            .open_on_demand()
            .await
            .expect("the connection opens");
        let messages = app
            .runtime()
            .opener::<WebsocketMessage>()
            .expect("the message scope is declared");
        let entries = served()
            .map(|handler| {
                let entry = messages
                    .entry(handler.key())
                    .expect("the handler's entry resolves");

                (handler, entry)
            })
            .collect();

        Self {
            _app: app,
            connection,
            messages,
            entries,
        }
    }

    async fn message(&self, handler: Handler) -> bool {
        let entry = self
            .entries
            .iter()
            .find_map(|(served, entry)| (*served == handler).then_some(entry))
            .expect("every served handler has an entry");
        let scope = self
            .messages
            .builder(WebsocketMessage)
            .under(&self.connection)
            .open_when_needed(Some(entry))
            .await
            .expect("the message scope opens");

        handler.run(scope).await
    }
}

/// What one message to `handler` allocates in an app with `count` message-scoped components.
fn measured(count: usize, handler: Handler) -> Delta {
    let runtime = runtime();
    let messages = runtime.block_on(Messages::build(count));
    per_call(&runtime, || messages.message(handler))
}

#[test]
fn unused_message_scoped_components_cost_a_message_nothing() {
    let without = measured(0, Handler::Hello);
    let with_unused = measured(UNUSED, Handler::Hello);

    assert_eq!(with_unused, without);
}

#[test]
fn a_message_injecting_one_component_costs_its_plan_not_the_scope() {
    let alone = measured(1, Handler::Scoped);
    let among_unused = measured(1 + UNUSED, Handler::Scoped);

    assert_eq!(
        among_unused, alone,
        "{UNUSED} unused message-scoped components changed what a message injecting one allocates"
    );
}

/// Opening a message scope always allocates its container, so a message that opens none makes
/// as many allocations as one injecting nothing.
#[test]
fn a_message_injecting_a_singleton_opens_no_scope() {
    let hello = measured(UNUSED, Handler::Hello);
    let clock = measured(UNUSED, Handler::Clock);

    assert_eq!(clock.allocations, hello.allocations);
}
