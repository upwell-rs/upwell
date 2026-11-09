use upwell::{Scope, ScopeId, component};

struct Local;

impl Scope for Local {
    const ID: ScopeId = upwell::namespaced_id!(ScopeId, "test/ui-list-local");
    const RANK: u8 = 1;
    const NAME: &'static str = "Local";
}

/// A component listing one scope twice.
#[component(scope = [Local, Local])]
struct Twice;

fn main() {}
