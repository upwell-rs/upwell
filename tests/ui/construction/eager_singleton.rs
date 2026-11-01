use upwell::component;

/// An eager component of the root scope, named by its path.
#[component(scope = upwell::scope::Singleton, eager)]
struct Audit;

fn main() {}
