use upwell::{component, when};

#[component]
#[when]
pub struct NoExpression;

#[component]
#[when(true false)]
pub struct TwoExpressions;

#[component]
#[when(id = "named")]
pub struct OnlyAnId;

fn main() {}
