use upwell::scope;

#[scope(generations = durable)]
struct NoParents;

fn main() {}
