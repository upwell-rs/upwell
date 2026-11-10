use upwell::{component, methods};

#[component]
pub struct Store;

#[methods]
impl Store {
    #[init(scope = [request, Request])]
    fn build() -> Self {
        Self
    }
}

fn main() {}
