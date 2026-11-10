use upwell::scope::Root;
use upwell::scope;

#[scope(parents = [Root], rank = 200)]
struct Outer;

#[scope(parents = [Outer], rank = 200)]
struct SameRank;

fn main() {}
