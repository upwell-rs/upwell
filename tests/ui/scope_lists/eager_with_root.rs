use upwell::{Scope, ScopeId, component};

struct Local;

impl Scope for Local {
    const ID: ScopeId = upwell::namespaced_id!(ScopeId, "test/ui-list-eager-root");
    const RANK: u8 = 1;
    const NAME: &'static str = "Local";
}

/// An eager component listing the root beside a scope below it.
#[component(scope = [Local, Singleton], eager)]
struct Audit;

fn main() {}
