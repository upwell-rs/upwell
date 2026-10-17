use upwell::{component, when};

#[component]
#[when(id = "enabled", true)]
#[when(id = "enabled", !false)]
pub struct ExplicitTwice;

#[component]
#[when(true)]
#[when(true)]
pub struct SameExpression;

#[component]
#[when(id = "when", true)]
#[when(!false)]
pub struct RootId;

fn main() {}
