//! Entry keys derived for HTTP routes and WebSocket message handlers whose controller ID and
//! handler name are not already `snake_case`.

#![allow(non_snake_case)]

use upwell::axum::prelude::*;
use upwell::prelude::*;
use upwell_test_utils::TestEnvironment;

app! {
    /// Generated host for entry-key derivation tests.
    app EntryKeysTestApplication {
        name: "entry-keys-http",
        protocol: upwell::axum::Axum,
    }
}

/// A controller whose spaced, capitalized ID still forms an entry key.
#[controller(id = "User Accounts", path = "/users")]
struct UserAccounts;

#[handlers]
impl UserAccounts {
    #[get("/")]
    async fn listAll() -> String {
        String::from("everyone")
    }
}

/// A WebSocket controller whose spaced, capitalized ID still forms an entry key.
#[controller(id = "Chat Room", ws = JsonWs)]
struct ChatRoom;

#[handlers(ws = JsonWs)]
impl ChatRoom {
    #[message("hello")]
    async fn sayHello(&self) {}
}

#[test]
fn controller_ids_and_handler_names_normalize_into_entry_keys() {
    let environment = TestEnvironment::new("upwell-http-entry-keys-");
    let prepared = EntryKeysTestApplication::builder()
        .expect("app builder")
        .config_source(environment.config())
        .directories(environment.directories())
        .register_ws::<JsonWs>("/ws")
        .prepare()
        .expect("app prepares");

    let keys = prepared
        .entries()
        .entries()
        .iter()
        .map(|entry| entry.key.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        keys,
        ["http/user_accounts.list_all", "ws/chat_room.say_hello"]
    );
}
