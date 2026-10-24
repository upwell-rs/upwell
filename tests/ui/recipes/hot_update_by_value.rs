use upwell::{HotUpdate, Previous, component, methods};

#[component(by_value)]
#[derive(Clone)]
pub struct Banner;

#[methods]
impl Banner {
    #[hot_update]
    async fn relabel(_previous: Previous<Self>) -> HotUpdate<Self> {
        HotUpdate::new(|_: &Self| {})
    }
}

fn main() {}
