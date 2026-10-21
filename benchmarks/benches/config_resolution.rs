//! Config resolution hot paths — the work done on every config reload.
//!
//! - `templating_by_tree_size`: `from_value` deserializing a value tree while resolving `${..}`
//!   placeholders, across trees of 4/32/256 leaves (a mix of static, full-placeholder, and
//!   interpolated strings). This is the interpolation pass that reruns on every reload.
//! - `get_config_vs_get`: `ConfigManager::get_config` (default seeding plus the two value-tree
//!   clones it performs) versus `get` (no defaults, no clones) — the price of the defaulting layer.

use std::collections::HashMap;
use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use serde::Deserialize;
use upwell_config::{
    ConfigManager, ConfigProperties, ConfigStr, ConfigValue, DefaultSpec, MapResolver, Resolver,
    ResolverChain, Toml, from_value,
};

/// Builds a table of `n` string leaves: a third static, a third full `${VAR}` placeholders (which
/// resolve to a scalar), a third interpolated `a${VAR}-i` templates (which resolve to a string).
fn tree(n: usize) -> ConfigValue {
    let mut entries = Vec::with_capacity(n);

    for i in 0..n {
        let raw = match i % 3 {
            0 => format!("static-value-{i}"),
            1 => "${VAR}".to_string(),
            _ => format!("prefix-${{VAR}}-suffix-{i}"),
        };

        entries.push((
            format!("key{i}"),
            ConfigValue::Str(ConfigStr::parse(&raw).unwrap()),
        ));
    }

    ConfigValue::Table(entries)
}

fn resolver_chain() -> ResolverChain {
    let mut map = HashMap::new();
    map.insert("VAR".to_string(), "resolved".to_string());

    ResolverChain(vec![Box::new(MapResolver(map))])
}

fn templating_by_tree_size(c: &mut Criterion) {
    let chain = resolver_chain();

    let mut group = c.benchmark_group("config_templating");

    for leaves in [4usize, 32, 256] {
        let root = tree(leaves);

        group.bench_with_input(
            BenchmarkId::from_parameter(leaves),
            &root,
            |bencher, root| {
                bencher.iter(|| {
                    let resolved: HashMap<String, String> =
                        from_value(black_box(root), black_box(&chain)).expect("resolves");

                    black_box(resolved)
                });
            },
        );
    }

    group.finish();
}

#[derive(Deserialize)]
#[allow(dead_code)] // fields are read by serde during deserialization, not by the bench
struct DbConfig {
    host: String,
    port: u16,
    pool: u32,
    url: String,
}

/// Hand-implemented the way the `#[config]` macro would, supplying defaults for the fields the TOML
/// omits (including a templated `url` that resolves against the other values).
impl ConfigProperties for DbConfig {
    const NAME: &'static str = "DbConfig";
    const DEFAULTS: DefaultSpec = DefaultSpec::Fields(&[
        ("port", "5432"),
        ("pool", "16"),
        ("url", "postgres://${db.host}:${db.port}/main"),
    ]);
}

/// The same shape without the defaulting layer — resolved through plain `get`.
#[derive(Deserialize)]
#[allow(dead_code)] // fields are read by serde during deserialization, not by the bench
struct PlainDb {
    host: String,
    port: u16,
    pool: u32,
    url: String,
}

const DB_TOML: &str = r#"
[db]
host = "db.internal"
port = 6543
pool = 32
url = "postgres://db.internal:6543/main"
"#;

fn get_config_vs_get(c: &mut Criterion) {
    let manager = ConfigManager::<Toml>::from_str(DB_TOML)
        .expect("parses")
        .with_resolver(Box::new(MapResolver(HashMap::new())) as Box<dyn Resolver>);

    let mut group = c.benchmark_group("config_binding");

    group.bench_function("get_config_with_defaults", |bencher| {
        bencher.iter(|| black_box(manager.get_config::<DbConfig>("db").expect("binds")));
    });

    group.bench_function("get_plain", |bencher| {
        bencher.iter(|| black_box(manager.get::<PlainDb>("db").expect("binds")));
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .warm_up_time(Duration::from_secs(2))
        .measurement_time(Duration::from_secs(5))
        .sample_size(60);
    targets = templating_by_tree_size, get_config_vs_get
}
criterion_main!(benches);
