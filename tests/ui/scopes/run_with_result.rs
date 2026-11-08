use upwell::scope::Root;
use upwell::{ScopeOpener, scope};

#[scope(parents = [Root])]
struct Customer;

async fn run(customers: &ScopeOpener<Customer>) {
    let _ = customers
        .builder(Customer)
        .run(|| async { Ok::<_, std::io::Error>(()) })
        .await;
}

fn main() {}
