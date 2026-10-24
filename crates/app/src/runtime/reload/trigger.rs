//! Automatic reload triggers drive the runtime's transactional reload.

use std::path::PathBuf;

use upwell_config::{ReloadCause, ReloadSummary, ReloadTarget};

use crate::AppRuntime;

impl ReloadTarget for AppRuntime {
    type Error = crate::Error;

    fn sources(&self) -> Vec<PathBuf> {
        self.reloader.sources()
    }

    async fn trigger_reload(&self, cause: ReloadCause) -> Result<ReloadSummary, Self::Error> {
        let report = self.reload_for(cause).await?;

        Ok(ReloadSummary {
            generation: report.config_generation(),
            changed: report.changed().len(),
        })
    }
}
