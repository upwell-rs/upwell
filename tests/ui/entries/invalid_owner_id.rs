use upwell::daemon::{handlers, service};

/// A service whose ID has no ASCII `snake_case` form.
#[service(id = "日本")]
struct Calc;

#[handlers]
impl Calc {
    #[rpc]
    async fn ping() -> u32 {
        1
    }
}

fn main() {}
