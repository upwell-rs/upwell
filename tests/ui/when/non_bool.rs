use serde::Deserialize;
use upwell::{component, config, when};

#[config(path = "server")]
#[derive(Deserialize)]
pub struct ServerConfig {
    pub port: u16,
}

#[component]
#[when(ServerConfig.port)]
pub struct Listener;

fn main() {}
