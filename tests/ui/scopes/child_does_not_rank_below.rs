use upwell::axum::HttpRequest;
use upwell::scope;
use upwell::scope::Root;

#[scope(parents = [Root], rank = 50, children = [HttpRequest])]
struct Session;

fn main() {}
