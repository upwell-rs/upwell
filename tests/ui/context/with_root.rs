use upwell::scope::Root;
use upwell::{Scope, ScopeId, context};

struct Local;

impl Scope for Local {
    const ID: ScopeId = upwell::namespaced_id!(ScopeId, "test/ui-context-root");
    const RANK: u8 = 1;
    const NAME: &'static str = "Local";
}

/// A context listing the root beside a scope below it.
#[context(scopes = [Local, Root])]
#[derive(Clone)]
struct Identity;

fn main() {}
