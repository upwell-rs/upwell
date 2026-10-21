use upwell::{Previous, component, methods};

#[component]
pub struct Ledger;

#[methods]
impl Ledger {
    #[replace]
    async fn carry(_previous: Previous<Self>) -> Self {
        Self
    }
}

#[methods]
impl Ledger {
    #[replace]
    async fn carry_again(_previous: Previous<Self>) -> Self {
        Self
    }
}

fn main() {}
