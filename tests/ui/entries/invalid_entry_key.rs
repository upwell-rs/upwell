use upwell::entry;

/// An entry whose explicit key has a single segment.
#[entry(scope = upwell::scope::Singleton, key = "reconcile")]
async fn reconcile() {}

fn main() {}
