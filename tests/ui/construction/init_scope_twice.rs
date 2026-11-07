use upwell::{Scope, ScopeId, component, methods};

struct Local;

impl Scope for Local {
    const ID: ScopeId = upwell::namespaced_id!(ScopeId, "test/ui-local");
    const RANK: u8 = 1;
    const NAME: &'static str = "Local";
}

#[component]
pub struct Store;

#[methods]
impl Store {
    #[init(scope = Local, scope = Local)]
    fn build() -> Self {
        Self
    }
}

fn main() {}
