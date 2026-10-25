use upwell::daemon::{handlers, service};

/// A service whose ID cannot form an entry key.
#[service(id = "Calc Service")]
struct Calc;

#[handlers]
impl Calc {
    #[rpc]
    async fn ping() -> u32 {
        1
    }
}

fn main() {}
