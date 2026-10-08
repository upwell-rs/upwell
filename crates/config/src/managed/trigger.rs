//! Automatic config-reload triggers.
//!
//! A [`ConfigManager`] may request reloads on `SIGHUP` (Unix) or on config-file changes (the
//! `watch` feature). The daemon spawns the matching background tasks at `serve`/`run` and
//! aborts them on shutdown; each drives one [`ReloadTarget`] and logs the outcome. The
//! application runtime is the target, so triggered reloads take the same transactional
//! config-and-graph path as a manual reload.
//!
//! [`ConfigManager`]: super::ConfigManager

use std::fmt::Display;
use std::path::PathBuf;

#[cfg(any(unix, feature = "watch"))]
use futures::FutureExt;
use tokio::task::JoinHandle;
use tracing::error;
#[cfg(any(unix, feature = "watch"))]
use tracing::{info, warn};

use super::ReloadTriggers;

/// A reload operation that the automatic `SIGHUP` and file-watch triggers drive.
///
/// The application runtime implements it with its transactional config-and-graph reload, so
/// triggered reloads never bypass the runtime transition coordinator.
pub trait ReloadTarget: Clone + Send + Sync + 'static {
    /// The error a failed reload reports.
    type Error: Display + Send;

    /// A snapshot of the config source files a file watcher observes.
    fn sources(&self) -> Vec<PathBuf>;

    /// Runs one complete reload.
    fn trigger_reload(&self) -> impl Future<Output = Result<ReloadSummary, Self::Error>> + Send;
}

/// The loggable outcome of one triggered reload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReloadSummary {
    /// The config generation after the reload.
    pub generation: u64,
    /// How many bindings changed and were re-published.
    pub changed: usize,
}

/// Spawns the background tasks for the requested triggers, returning their handles so the
/// caller can abort them on shutdown. Unsupported requests (SIGHUP off-Unix, watching
/// without the `watch` feature) are logged and skipped.
#[allow(unused_mut, unused_variables)]
pub fn spawn_reload_triggers<T: ReloadTarget>(
    target: T,
    triggers: ReloadTriggers,
) -> Vec<JoinHandle<()>> {
    let mut handles = Vec::new();

    if triggers.sighup {
        #[cfg(unix)]
        handles.push(spawn_sighup(target.clone()));

        #[cfg(not(unix))]
        tracing::warn!(target: "upwell::config", "reload_on_sighup is Unix-only; ignoring");
    }

    if triggers.watch {
        #[cfg(feature = "watch")]
        if let Some(handle) = spawn_watch(target.clone(), triggers.debounce) {
            handles.push(handle);
        }

        #[cfg(not(feature = "watch"))]
        tracing::warn!(
            target: "upwell::config",
            "watch_config requires the `watch` feature; ignoring"
        );
    }

    handles
}

/// Cancels every reload trigger and waits until each task has actually exited.
/// Awaiting aborted tasks gives callers a deterministic no-lingering-task boundary.
pub async fn stop_reload_triggers(handles: Vec<JoinHandle<()>>) {
    for handle in &handles {
        handle.abort();
    }

    for handle in handles {
        match handle.await {
            Ok(()) => {}
            Err(error) if error.is_cancelled() => {}
            Err(error) => error!(target: "upwell::config", %error, "reload trigger task failed"),
        }
    }
}

/// Runs one reload and logs its outcome.
#[cfg(any(unix, feature = "watch"))]
async fn run_reload<T: ReloadTarget>(target: &T, cause: &'static str) {
    match std::panic::AssertUnwindSafe(target.trigger_reload())
        .catch_unwind()
        .await
    {
        Ok(Ok(report)) => info!(
            target: "upwell::config",
            cause,
            generation = report.generation,
            changed = report.changed,
            "configuration reloaded"
        ),

        Ok(Err(error)) => error!(
            target: "upwell::config",
            cause,
            %error,
            "configuration reload failed"
        ),

        Err(_) => error!(
            target: "upwell::config",
            cause,
            "configuration reload panicked; trigger remains active"
        ),
    }
}

/// Reloads whenever the process receives `SIGHUP`.
#[cfg(unix)]
fn spawn_sighup<T: ReloadTarget>(target: T) -> JoinHandle<()> {
    use tokio::signal::unix::{SignalKind, signal};

    tokio::spawn(async move {
        let mut hangup = match signal(SignalKind::hangup()) {
            Ok(hangup) => hangup,

            Err(error) => {
                error!(target: "upwell::config", %error, "failed to install SIGHUP handler");

                return;
            }
        };

        info!(target: "upwell::config", "reloading configuration on SIGHUP");

        while hangup.recv().await.is_some() {
            run_reload(&target, "sighup").await;
        }

        warn!(target: "upwell::config", "SIGHUP reload trigger stopped unexpectedly");
    })
}

/// Reloads when any config source file changes, coalescing bursts over the debounce window.
/// Watches the source files' parent directories, since editors and atomic writes replace the
/// file (which would drop a file-level watch).
#[cfg(feature = "watch")]
fn spawn_watch<T: ReloadTarget>(
    target: T,
    debounce: std::time::Duration,
) -> Option<JoinHandle<()>> {
    use std::collections::HashSet;
    use std::path::Path;

    use notify::{RecursiveMode, Watcher};

    let sources = target.sources();

    if sources.is_empty() {
        tracing::warn!(
            target: "upwell::config",
            "watch_config enabled but there are no config sources to watch"
        );

        return None;
    }

    let dirs: HashSet<PathBuf> = sources
        .iter()
        .filter_map(|source| source.parent().map(Path::to_path_buf))
        .collect();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();

    let mut watcher =
        match notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
            if result.is_ok() {
                let _ = tx.send(());
            }
        }) {
            Ok(watcher) => watcher,

            Err(error) => {
                error!(target: "upwell::config", %error, "failed to create config file watcher");

                return None;
            }
        };

    for dir in &dirs {
        if let Err(error) = watcher.watch(dir, RecursiveMode::NonRecursive) {
            error!(
                target: "upwell::config",
                dir = %dir.display(),
                %error,
                "failed to watch config directory"
            );
        }
    }

    let watched = dirs.len();

    Some(tokio::spawn(async move {
        // Hold the watcher for the task's lifetime; dropping it stops watching.
        let _watcher = watcher;

        info!(target: "upwell::config", dirs = watched, "watching config files for changes");

        while rx.recv().await.is_some() {
            tokio::time::sleep(debounce).await;

            while rx.try_recv().is_ok() {}

            run_reload(&target, "file-change").await;
        }

        warn!(target: "upwell::config", "config file reload trigger stopped unexpectedly");
    }))
}

#[cfg(test)]
mod tests;
