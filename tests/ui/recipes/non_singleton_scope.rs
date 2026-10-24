use upwell::scope::Transient;
use upwell::{Previous, component, methods};

#[component(scope = transient)]
pub struct Ledger;

#[methods]
impl Ledger {
    #[replace]
    async fn carry(_previous: Previous<Self>) -> Self {
        Self
    }
}

fn main() {}
