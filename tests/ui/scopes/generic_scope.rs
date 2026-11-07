use upwell::scope::Root;
use upwell::scope;

#[scope(parents = [Root])]
struct Generic<T>(T);

fn main() {}
