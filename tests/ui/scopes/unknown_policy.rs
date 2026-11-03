use upwell::scope::Root;
use upwell::scope;

#[scope(parents = [Root], generations = forever)]
struct Forever;

fn main() {}
