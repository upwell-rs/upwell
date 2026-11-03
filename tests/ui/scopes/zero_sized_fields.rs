use upwell::scope::Root;
use upwell::scope;

#[scope(parents = [Root])]
struct Flag {
    marker: (),
}

fn main() {}
