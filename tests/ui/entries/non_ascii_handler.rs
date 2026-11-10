use upwell::daemon::{handlers, service};

/// A service whose method name has no ASCII `snake_case` form.
#[service(id = "calc")]
struct Calc;

#[handlers]
impl Calc {
    #[rpc]
    async fn grüßen() -> u32 {
        1
    }
}

fn main() {}
