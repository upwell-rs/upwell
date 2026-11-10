use upwell::context;

/// A context listing no scope at all.
#[context(scopes = [])]
#[derive(Clone)]
struct Identity;

fn main() {}
