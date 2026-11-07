use upwell::component;

#[component]
pub struct Identity;

#[component]
pub struct Desk {
    #[config]
    identity: upwell::context::Current<std::sync::Arc<Identity>>,
}

fn main() {}
