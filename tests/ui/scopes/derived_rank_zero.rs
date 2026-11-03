use upwell::scope::Root;
use upwell::scope;

#[scope(parents = [Root], rank = 1)]
struct Lowest;

#[scope(parents = [Lowest])]
struct BelowLowest;

fn main() {}
