use serde::Deserialize;
use upwell::{component, config, when};

#[config(path = "audit")]
#[derive(Deserialize)]
pub struct AuditConfig {
    pub enabled: bool,
}

#[component]
pub struct AuditTransport {
    #[default]
    pub enabled: bool,
}

#[component]
#[when(AuditTransport.enabled)]
pub struct ComponentFieldRead;

#[component]
#[when(AuditConfig.eligible())]
pub struct ConfigEligibility;

fn main() {}
