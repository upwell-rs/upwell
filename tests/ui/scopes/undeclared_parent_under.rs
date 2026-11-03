use upwell::scope::Root;
use upwell::{OpenScope, ScopeOpener, scope};

#[scope(parents = [Root])]
struct Customer;

#[scope(parents = [Root])]
struct Other;

#[scope(parents = [Customer])]
struct Batch;

fn open(batches: &ScopeOpener<Batch>, other: &OpenScope<Other>) {
    let _ = batches.builder(Batch).under(other);
}

fn main() {}
