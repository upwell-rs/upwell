use serde::Deserialize;
use upwell::{component, config, when};

pub struct Settings {
    pub enabled: bool,
}

#[config]
#[derive(Deserialize)]
pub struct Unbound {
    pub enabled: bool,
}

#[component]
#[when(Settings.enabled)]
pub struct PlainType;

#[component]
#[when(Unbound.enabled)]
pub struct ConfigWithoutPath;

fn main() {}
