use upwell::{Scope, ScopeId, component};

struct Local;

impl Scope for Local {
    const ID: ScopeId = upwell::namespaced_id!(ScopeId, "test/ui-list-not-a-scope");
    const RANK: u8 = 1;
    const NAME: &'static str = "Local";
}

/// A component listing a type that is not a scope.
#[component(scope = [Local, String])]
struct Misplaced;

fn main() {}
