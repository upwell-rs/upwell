use upwell::scope;
use upwell::scope::Root;

#[scope(parents = [Root], children = [Missing])]
struct Session;

fn main() {}
