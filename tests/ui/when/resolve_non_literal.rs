use upwell::condition::resolve;
use upwell::{component, when};

const FLAG: &str = "features.flag";

#[component]
#[when(resolve(FLAG) == Some(true))]
pub struct Constant;

#[component]
#[when(resolve::<bool>() == Some(true))]
pub struct NoPath;

fn main() {}
