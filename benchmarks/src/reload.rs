//! File-backed applications over a [`graph`](crate::graph) roster, for measuring one whole
//! reload transaction: staging, condition evaluation, candidate preparation, transition
//! planning, candidate construction, and commit.
//!
//! The application binds one `bench` config section with two fields. `gated` is a condition
//! fact that one extra component, [`Gated`], is enabled by; `unrelated` is read by nothing.
//! A [`Change`] rewrites the config file before the reload so each scenario isolates one
//! kind of change while the roster nodes themselves never depend on config.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use upwell_app::App;
use upwell_config::{ConditionFacts, ConfigManager, ConfigProperties, ResolverChain, Toml};
use upwell_core::{
    ConditionDescriptor, ConditionPredicate, ConditionScalar, ConditionScalarKind,
    ConfigFactDescriptor, ConfigFactId,
};
use upwell_di::{Component, ComponentDescriptor, factories};

const BINDING: &str = "bench";

const GATED_FACT: ConfigFactId = ConfigFactId::new("bench::BenchConfig", BINDING, "gated");

/// The config section every reload re-stages.
#[derive(Deserialize)]
pub struct BenchConfig {
    gated: bool,
    #[allow(dead_code, reason = "changing it is the unrelated-change scenario")]
    unrelated: i64,
}

impl ConfigProperties for BenchConfig {
    const NAME: &'static str = "BenchConfig";
}

impl ConditionFacts for BenchConfig {
    fn condition_facts(binding_path: &'static str) -> Vec<ConfigFactDescriptor> {
        vec![ConfigFactDescriptor {
            id: ConfigFactId::new("bench::BenchConfig", binding_path, "gated"),
            kind: ConditionScalarKind::Bool,
            source: upwell_core::descriptor_source!(),
        }]
    }

    fn condition_scalars(
        &self,
        binding_path: &'static str,
    ) -> Vec<(ConfigFactId, ConditionScalar)> {
        vec![(
            ConfigFactId::new("bench::BenchConfig", binding_path, "gated"),
            ConditionScalar::Bool(self.gated),
        )]
    }
}

/// A factory-backed singleton enabled only while `bench.gated` is true.
pub struct Gated;

impl Component for Gated {
    type Handle = Arc<Self>;

    const ID: &'static str = "bench-gated";
    const NAME: &'static str = "Gated";

    fn into_handle(self) -> Arc<Self> {
        Arc::new(self)
    }
}

async fn gated() -> Gated {
    Gated
}

static GATED_CONDITION: ConditionDescriptor = ConditionDescriptor {
    id: "bench-gated",
    source: upwell_core::descriptor_source!(),
    predicate: ConditionPredicate::ConfigBool(GATED_FACT),
};

static GATED: ComponentDescriptor = ComponentDescriptor::of::<Gated>()
    .with_factories(factories![default "static" => gated])
    .with_condition(&GATED_CONDITION);

/// What the next reload sees changed in the config file.
#[derive(Clone, Copy, Debug)]
pub enum Change {
    /// Nothing: the file is rewritten with identical contents.
    None,
    /// Only `bench.unrelated`, which no condition or component reads.
    Unrelated,
    /// `bench.gated`, which flips the one conditional component.
    Condition,
}

/// A built application and the config file its reloads read.
pub struct ReloadApp {
    app: App<()>,
    dir: PathBuf,
    gated: bool,
    unrelated: i64,
}

impl ReloadApp {
    /// Builds an application of `nodes` roster nodes plus [`Gated`] in a fresh directory
    /// under the system temp dir, keyed by `label` so concurrent benches do not collide.
    pub async fn build(label: &str, nodes: usize) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "upwell-reload-bench-{label}-{nodes}-{}",
            std::process::id()
        ));

        std::fs::create_dir_all(&dir).expect("create bench config dir");
        write_config(&dir, true, 0);

        let manager =
            ConfigManager::<Toml>::load_in_with_resolvers(&dir, &[], ResolverChain::empty())
                .expect("load bench config");
        let builder = crate::graph::components(nodes).iter().fold(
            App::<()>::builder("reload-bench")
                .config_source(manager)
                .config::<BenchConfig>(BINDING)
                .condition_facts::<BenchConfig>(BINDING)
                .component_descriptor(&GATED),
            |builder, node| builder.component_descriptor(node),
        );
        let app = builder.build().await.expect("bench app builds");

        Self {
            app,
            dir,
            gated: true,
            unrelated: 0,
        }
    }

    /// Rewrites the config file so the next reload observes `change`.
    pub fn stage(&mut self, change: Change) {
        match change {
            Change::None => {}
            Change::Unrelated => self.unrelated += 1,
            Change::Condition => self.gated = !self.gated,
        }

        write_config(&self.dir, self.gated, self.unrelated);
    }

    /// Runs one reload transaction and panics if it does not commit.
    pub async fn reload(&self) {
        self.app
            .reloader()
            .reload()
            .await
            .expect("bench reload commits");
    }
}

impl Drop for ReloadApp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn write_config(dir: &Path, gated: bool, unrelated: i64) {
    std::fs::write(
        dir.join("application.toml"),
        format!("[{BINDING}]\ngated = {gated}\nunrelated = {unrelated}\n"),
    )
    .expect("write bench config");
}
