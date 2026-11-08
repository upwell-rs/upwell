use upwell::{ScopeEnd, ScopeEnded, component, methods};

struct NotAScope;

#[component]
struct Observer {}

#[methods]
impl Observer {
    #[hook(ScopeEnded<NotAScope>)]
    async fn ended(&self, _end: ScopeEnd) {}
}

fn main() {}
