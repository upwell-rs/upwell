use upwell::component;

/// The transient scope under another name, which only the scope type itself reveals.
type PerResolution = upwell::scope::Transient;

/// An eager component of the transient scope.
#[component(scope = PerResolution, eager)]
struct Audit;

fn main() {}
