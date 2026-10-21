use std::sync::Arc;

use upwell::{component, methods};

#[component]
pub struct Clock;

#[component]
pub struct Ledger {
    clock: Arc<Clock>,
}

#[methods]
impl Ledger {
    #[replace]
    async fn carry(clock: Arc<Clock>) -> Self {
        Self { clock }
    }
}

fn main() {}
