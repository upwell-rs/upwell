use upwell::scope::Root;
use upwell::{ScopeOpener, scope};

#[scope(parents = [Root])]
struct Customer;

#[scope(parents = [Customer])]
struct Batch;

async fn run(batches: &ScopeOpener<Batch>) {
    let _ = batches.builder(Batch).run(|| async {}).await;
}

fn main() {}
