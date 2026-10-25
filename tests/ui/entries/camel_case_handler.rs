use upwell::daemon::{handlers, service};

/// A service whose method name cannot form an entry key.
#[service(id = "calc")]
struct Calc;

#[handlers]
impl Calc {
    #[rpc]
    #[allow(non_snake_case)]
    async fn pingAll() -> u32 {
        1
    }
}

fn main() {}
