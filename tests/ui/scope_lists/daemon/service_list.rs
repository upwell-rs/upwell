use upwell::daemon::{handlers, service};
use upwell::{Scope, ScopeId};

struct Local;

impl Scope for Local {
    const ID: ScopeId = upwell::namespaced_id!(ScopeId, "test/ui-list-service");
    const RANK: u8 = 1;
    const NAME: &'static str = "Local";
}

/// A service listing scopes, though services are singletons.
#[service(scope = [Singleton, Local])]
struct Greeter;

#[handlers]
impl Greeter {
    async fn greet(&self) -> String {
        String::new()
    }
}

fn main() {}
