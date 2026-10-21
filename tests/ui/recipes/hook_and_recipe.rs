use upwell::{ComponentRetirement, Previous, component, methods};

#[component]
pub struct Ledger;

#[methods]
impl Ledger {
    #[hook(ComponentRetired)]
    #[replace]
    async fn carry(_previous: Previous<Self>, _retirement: ComponentRetirement) -> Self {
        Self
    }
}

fn main() {}
