use upwell::{Previous, component, methods};

#[component]
pub struct Ledger;

#[methods]
impl Ledger {
    #[replace("carry")]
    async fn carry(_previous: Previous<Self>) -> Self {
        Self
    }
}

fn main() {}
