//! Regression coverage for reload invalidation and panic recovery.
#![cfg(feature = "daemon")]
#![allow(dead_code)]

use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Deserializer};
use tempfile::TempDir;
use upwell::config::Toml;
use upwell::daemon::App;
use upwell::{
    AppError, Cfg, CfgNext, ConfigManager, ConfigProperties, ConfigReload, ConfigReloadError,
    HookOutcome, component, config, methods,
};
use upwell_config::{Resolver, ResolverChain};

fn temp_config(tag: &str, contents: &str) -> (TempDir, PathBuf) {
    let root = tempfile::Builder::new()
        .prefix(&format!("upwell-reload-hardening-{tag}-"))
        .tempdir()
        .expect("create config directory");
    let config = root.path().join("application.toml");

    fs::write(&config, contents).expect("write config");

    (root, config)
}

/// Asserts that a rejected reload's report never renders the sensitive marker.
fn assert_redacted_report(error: &AppError) {
    let AppError::TransitionRejected(rejection) = error else {
        panic!("expected a rejected transition, got {error:?}");
    };

    assert!(!format!("{:?}", rejection.report()).contains("sensitive"));
    assert!(!rejection.to_string().contains("sensitive"));
}

mod invalidation;
mod panics;
