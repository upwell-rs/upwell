mod settings {
    use serde::Deserialize;
    use upwell::config;

    #[config(path = "audit")]
    #[derive(Deserialize)]
    pub struct AuditConfig {
        enabled: bool,
    }
}

use settings::AuditConfig;
use upwell::{component, when};

#[component]
#[when(AuditConfig.enabled)]
pub struct Auditor;

fn main() {}
