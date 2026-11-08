use upwell::axum::HttpRequest;
use upwell::scope;
use upwell::scope::Root;

#[scope(parents = [Root], children = [HttpRequest, HttpRequest])]
struct Session;

fn main() {}
