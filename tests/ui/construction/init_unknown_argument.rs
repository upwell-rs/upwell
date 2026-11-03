use upwell::{component, methods};

#[component]
pub struct Store;

#[methods]
impl Store {
    #[init(qualifier = "main")]
    fn build() -> Self {
        Self
    }
}

fn main() {}
