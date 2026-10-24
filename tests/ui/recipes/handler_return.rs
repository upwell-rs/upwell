use upwell::{ComponentChange, component, methods};

#[component]
pub struct Greeter;

#[methods]
impl Greeter {
    #[transition]
    fn pick(_change: &ComponentChange) -> Option<Self> {
        None
    }
}

fn main() {}
