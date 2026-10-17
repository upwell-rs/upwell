use upwell::scope::Transient;
use upwell::{component, when};

#[component(scope = transient)]
#[when(true)]
pub struct Rebuilt;

fn main() {}
