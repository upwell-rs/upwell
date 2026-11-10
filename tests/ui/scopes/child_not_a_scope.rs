use upwell::scope;
use upwell::scope::Root;

struct NotAScope;

#[scope(parents = [Root], children = [NotAScope])]
struct Session;

fn main() {}
