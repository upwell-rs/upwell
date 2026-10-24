use upwell::{ComponentChange, OneShot, component, methods};

#[component]
pub struct Greeter;

#[methods]
impl Greeter {
    #[transition]
    async fn pick(_change: &ComponentChange) -> Option<OneShot<Self>> {
        None
    }
}

fn main() {}
