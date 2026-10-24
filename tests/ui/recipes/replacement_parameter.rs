use upwell::{Previous, component, methods};

#[component]
pub struct Ledger;

#[methods]
impl Ledger {
    #[replace]
    async fn carry(_previous: Previous<Self>, _count: std::cell::Cell<u8>) -> Self {
        Self
    }
}

fn main() {}
