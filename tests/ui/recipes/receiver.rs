use upwell::{component, methods};

#[component]
pub struct Ledger;

#[methods]
impl Ledger {
    #[replace]
    async fn carry(&self) -> Self {
        Self
    }
}

fn main() {}
