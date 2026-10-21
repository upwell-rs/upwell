//! The recipe application: a `Mode` provider whose selection keys on `recipe.mode`, and one
//! consumer per transition recipe, each declared with the impl-block attribute macros.
//!
//! - [`Ledger`] carries its entry count across replacements with `#[replace]`;
//! - [`Greeter`] picks a one-shot factory with `#[transition]`;
//! - [`Banner`] relabels itself in place with `#[hot_update]`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use tempfile::TempDir;
use upwell::config::Toml;
use upwell::{
    App, Cfg, ComponentChange, ConfigManager, HotUpdate, OneShot, Previous, ReasonKind, component,
    config, injectable, methods, when,
};
use upwell_config::ResolverChain;

/// The behaviour the two providers switch between.
#[injectable]
pub trait Mode: Send + Sync {
    fn name(&self) -> &'static str;
}

#[config(path = "recipe")]
#[derive(Deserialize)]
pub struct RecipeConfig {
    pub mode: String,
    pub label: String,
}

/// The provider used unless `recipe.mode` is `"loud"`.
#[component(provide = dyn Mode, primary)]
#[when(RecipeConfig.mode != "loud")]
pub struct Calm;

impl Mode for Calm {
    fn name(&self) -> &'static str {
        "calm"
    }
}

/// The provider `recipe.mode = "loud"` selects.
#[component(provide = dyn Mode, primary)]
#[when(RecipeConfig.mode == "loud")]
pub struct Loud;

impl Mode for Loud {
    fn name(&self) -> &'static str {
        "loud"
    }
}

/// A fixed `Mode` consumer that carries its entry count across replacements.
#[component]
pub struct Ledger {
    pub mode: Arc<dyn Mode>,
    #[default]
    pub entries: usize,
}

#[methods]
impl Ledger {
    #[init]
    async fn open(mode: Arc<dyn Mode>) -> Self {
        Self { mode, entries: 0 }
    }

    #[replace]
    pub async fn carry(previous: Previous<Self>, mode: Arc<dyn Mode>) -> Self {
        Self {
            mode,
            entries: previous.entries + 1,
        }
    }
}

/// A fixed `Mode` consumer whose transition handler picks a one-shot factory.
#[component]
pub struct Greeter {
    pub mode: Arc<dyn Mode>,
    #[default]
    pub origin: String,
}

#[methods]
impl Greeter {
    #[init]
    fn new(mode: Arc<dyn Mode>) -> Self {
        Self {
            mode,
            origin: "ordinary".to_owned(),
        }
    }

    #[transition]
    pub fn pick(change: &ComponentChange) -> Option<OneShot<Self>> {
        let origin = if change.has_reason(ReasonKind::FixedDependent) {
            "one-shot:fixed-dependent"
        } else {
            "one-shot:other"
        };

        Some(OneShot::replacing(
            move |previous: Previous<Self>, mode: Arc<dyn Mode>| async move {
                Self {
                    mode,
                    origin: format!("{origin} after {}", previous.origin),
                }
            },
        ))
    }
}

/// A `recipe` config consumer that relabels itself in place.
#[component]
pub struct Banner {
    #[default]
    label: Mutex<String>,
}

impl Banner {
    pub fn label(&self) -> String {
        self.label.lock().expect("banner label lock").clone()
    }
}

#[methods]
impl Banner {
    #[init]
    async fn new(cfg: Cfg<RecipeConfig>) -> Self {
        Self {
            label: Mutex::new(cfg.snapshot().label.clone()),
        }
    }

    #[hot_update]
    pub async fn relabel(_previous: Previous<Self>, cfg: Cfg<RecipeConfig>) -> HotUpdate<Self> {
        let label = cfg.snapshot().label.clone();

        HotUpdate::new(move |banner: &Self| {
            *banner.label.lock().expect("banner label lock") = label;
        })
    }
}

pub fn write_config(config_dir: &Path, mode: &str, label: &str) {
    fs::write(
        config_dir.join("application.toml"),
        format!("[recipe]\nmode = \"{mode}\"\nlabel = \"{label}\"\n"),
    )
    .expect("write recipe config");
}

/// Builds the app over a fresh config directory.
pub async fn build(mode: &str, label: &str) -> (TempDir, PathBuf, App<()>) {
    let dir = tempfile::Builder::new()
        .prefix("upwell-transition-recipes-")
        .tempdir()
        .expect("create temp dir");
    let config_dir = dir.path().join("config");

    fs::create_dir_all(&config_dir).expect("create config dir");
    write_config(&config_dir, mode, label);

    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(&config_dir, &[], ResolverChain::empty())
            .expect("load config");
    let app = App::<()>::builder("transition-recipes-test")
        .config_source(manager)
        .config::<RecipeConfig>("recipe")
        .component::<Calm>()
        .component::<Loud>()
        .component::<Ledger>()
        .component::<Greeter>()
        .component::<Banner>()
        .provider::<Calm>()
        .provider::<Loud>()
        .build()
        .await
        .expect("the recipe app builds");

    (dir, config_dir, app)
}
