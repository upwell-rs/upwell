use upwell::component;

#[component]
pub struct Identity;

#[component]
pub struct Desk {
    #[qualifier = "admin"]
    identity: upwell::context::Current<std::sync::Arc<Identity>>,
}

fn main() {}
