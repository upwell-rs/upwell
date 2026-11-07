//! Entry keys derived from Rust names and owner IDs that are not already `snake_case`.

#![cfg(feature = "daemon")]
#![allow(non_snake_case)]

use upwell::daemon::{App, handlers, service};

/// A service whose spaced, capitalized ID still forms an entry key.
#[service(id = "Calc Service")]
struct CalcService;

#[handlers]
impl CalcService {
    #[rpc]
    async fn pingAll() -> u32 {
        1
    }

    #[rpc]
    async fn r#type() -> u32 {
        2
    }
}

#[test]
fn owner_ids_and_handler_names_normalize_into_entry_keys() {
    let prepared = App::builder("entry-keys-test")
        .auto_discover()
        .prepare()
        .expect("app prepares");

    let keys = prepared
        .entries()
        .entries()
        .iter()
        .map(|entry| entry.key.as_str())
        .collect::<Vec<_>>();

    assert_eq!(keys, ["rpc/calc_service.ping_all", "rpc/calc_service.type"]);
}
