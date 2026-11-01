use upwell::component;

/// An eager component without a scope: singletons are already built at startup.
#[component(eager)]
struct Audit;

fn main() {}
