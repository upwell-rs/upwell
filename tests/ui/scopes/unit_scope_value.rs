use std::sync::Arc;

use upwell::scope::Root;
use upwell::{component, scope};

#[scope(parents = [Root])]
struct Unit;

#[component(scope = Unit)]
struct ReadsUnit {
    unit: Arc<Unit>,
}

fn main() {}
