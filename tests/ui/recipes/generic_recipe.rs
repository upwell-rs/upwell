use upwell::{Previous, component, methods};

#[component]
pub struct Ledger;

#[methods]
impl Ledger {
    #[replace]
    async fn carry<T: Default>(_previous: Previous<Self>) -> Self {
        let _ = T::default();

        Self
    }
}

fn main() {}
