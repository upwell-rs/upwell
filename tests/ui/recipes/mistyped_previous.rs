use upwell::{Previous, component, methods};

#[component]
pub struct Clock;

#[component]
pub struct Banner;

#[methods]
impl Banner {
    #[hot_update]
    async fn relabel(_previous: Previous<Clock>) -> upwell::HotUpdate<Self> {
        upwell::HotUpdate::new(|_: &Self| {})
    }
}

fn main() {}
