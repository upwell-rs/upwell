use upwell::{Scope, ScopeId, component};

struct Local;

impl Scope for Local {
    const ID: ScopeId = upwell::namespaced_id!(ScopeId, "test/ui-list-eager-transient");
    const RANK: u8 = 1;
    const NAME: &'static str = "Local";
}

/// The transient scope under another name, which only the scope type itself reveals.
type PerResolution = upwell::scope::Transient;

/// An eager component listing the transient scope under an alias.
#[component(scope = [Local, PerResolution], eager)]
struct Audit;

fn main() {}
