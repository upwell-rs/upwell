use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::de::DeserializeOwned;
use tracing::{debug, info, instrument, trace};
use upwell_dirs::DirectoriesManager;

use crate::resolve::{ResolveCtx, ResolvedDependency};
use crate::{ConfigValue, Resolver, ResolverChain, from_value_in};

use super::{CONFIG_BINDINGS, ConfigBinding, ConfigError, ConfigProperties, DirectoriesResolver};

/// A parser from source text to the normalized config tree.
type Parser = fn(&str) -> Result<ConfigValue, crate::TemplateError>;

/// Which automatic reload triggers a [`ConfigManager`] requests, beyond manual reloads through
/// the application runtime. The daemon reads these at `serve`/`run` and spawns the matching
/// background tasks.
#[derive(Clone, Copy, Debug)]
pub struct ReloadTriggers {
    /// Reload on `SIGHUP` (Unix only).
    pub sighup: bool,
    /// Watch the config source files and reload on change (requires the `watch` feature).
    pub watch: bool,
    /// How long to coalesce a burst of file-change events before reloading.
    pub debounce: Duration,
}

impl Default for ReloadTriggers {
    fn default() -> Self {
        Self {
            sighup: false,
            watch: false,
            debounce: Duration::from_millis(250),
        }
    }
}

/// Which source format(s) a [`ConfigManager`] reads. Retained as runtime data so the
/// daemon knows how to re-read on reload even after the `Format` type is erased.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatId {
    Toml,
    Yaml,
    Dynamic,
}

/// A config source format: the file extensions it owns (and their parsers), in
/// precedence order. The type parameter on [`ConfigManager`] is what makes
/// `ConfigManager<Toml>` only try `.toml` while `ConfigManager<Dynamic>` tries all.
pub trait Format: 'static {
    /// The runtime tag recorded on a manager built with this format.
    const ID: FormatId;

    /// The `(extension, parser)` pairs this format loads, highest precedence last
    /// (so later files override earlier on merge).
    fn parsers() -> Vec<(&'static str, Parser)>;
}

/// Reads only TOML (`application.toml`).
pub struct Toml;

impl Format for Toml {
    const ID: FormatId = FormatId::Toml;

    fn parsers() -> Vec<(&'static str, Parser)> {
        vec![("toml", crate::format::toml::from_str)]
    }
}

/// Reads only YAML (`application.yaml` / `.yml`).
#[cfg(feature = "yaml")]
pub struct Yaml;

#[cfg(feature = "yaml")]
impl Format for Yaml {
    const ID: FormatId = FormatId::Yaml;

    fn parsers() -> Vec<(&'static str, Parser)> {
        vec![
            ("yaml", crate::format::yaml::from_str),
            ("yml", crate::format::yaml::from_str),
        ]
    }
}

/// Reads every enabled format, letting the file extension decide the parser.
pub struct Dynamic;

impl Format for Dynamic {
    const ID: FormatId = FormatId::Dynamic;

    fn parsers() -> Vec<(&'static str, Parser)> {
        #[allow(unused_mut)]
        let mut parsers: Vec<(&'static str, Parser)> =
            vec![("toml", crate::format::toml::from_str)];

        #[cfg(feature = "yaml")]
        {
            parsers.push(("yaml", crate::format::yaml::from_str));
            parsers.push(("yml", crate::format::yaml::from_str));
        }

        parsers
    }
}

/// The merged configuration tree, owning the format it was read in.
///
/// Built by the application (typically in `main`) so values can configure the
/// transport *before* the daemon exists, then handed to the daemon builder to seed the
/// dependency-injection bindings. The `Format` type optimizes *what gets loaded*; it
/// is irrelevant once the tree is parsed, so the DI path uses the format-erased
/// [`ConfigManager<Dynamic>`]. The original format is retained as runtime data
/// ([`format`](Self::format)) so reload still knows how to re-read.
pub struct ConfigManager<F = Dynamic> {
    root: ConfigValue,
    resolvers: ResolverChain,
    format: FormatId,
    sources: Vec<PathBuf>,
    /// The config types bound to property paths (auto-discovered and/or explicit). The
    /// manager owns this registry so it can seed every bound type's defaults into the tree —
    /// the daemon reads it back at build to construct the `Cfg<T>` injectables.
    bindings: Vec<ConfigBinding>,
    /// Which automatic reload triggers this manager requests (manual reload is always on).
    triggers: ReloadTriggers,
    _marker: PhantomData<F>,
}

impl<F: Format> ConfigManager<F> {
    /// An empty configuration, for daemons that bind no config. Reads no files but retains
    /// environment placeholder resolution; replace the resolver chain for deterministic tests.
    pub fn empty() -> Self {
        Self::wrap(ConfigValue::Table(Vec::new()), Vec::new())
    }

    /// Parses one in-memory document in this format (the first parser wins for
    /// multi-extension formats). Handy for tests and embedded defaults.
    ///
    /// Named `from_str` to read as `ConfigManager::<Toml>::from_str`; it is a typed,
    /// fallible constructor, not the `std::str::FromStr` trait method.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(text: &str) -> Result<Self, ConfigError> {
        let parsers = F::parsers();
        let (_, parse) = parsers
            .first()
            .expect("a format defines at least one parser");

        let root = parse(text).map_err(|source| ConfigError::Parse {
            path: PathBuf::from("<in-memory>"),
            source,
        })?;

        Ok(Self::wrap(root, Vec::new()))
    }

    /// Discovers and deep-merges config from the directory `dir` (typically the config
    /// directory from a `DirectoriesManager`).
    ///
    /// Takes a plain [`Path`] rather than a `Dir<Config>`, so config need not name the
    /// directory kind types. The base `application.<ext>` underlies the per-profile overlays
    /// `application-<profile>.<ext>`, each overriding the previous. Profiles come from
    /// `UPWELL_PROFILES` (comma-separated) first, then `profiles`. A missing file is
    /// skipped; a malformed one is an error.
    #[instrument(target = "upwell::config", level = "debug", skip(dir, profiles), fields(dir = %dir.display()))]
    pub fn load_in(dir: &Path, profiles: &[String]) -> Result<Self, ConfigError> {
        Self::load_in_with_resolvers(dir, profiles, ResolverChain::env_default())
    }

    /// Like [`load_in`](Self::load_in), but uses an explicit resolver chain for both
    /// `UPWELL_PROFILES` and later placeholder substitution.
    #[instrument(target = "upwell::config", level = "debug", skip(dir, profiles, resolvers), fields(dir = %dir.display()))]
    pub fn load_in_with_resolvers(
        dir: &Path,
        profiles: &[String],
        resolvers: ResolverChain,
    ) -> Result<Self, ConfigError> {
        let active = resolve_profiles(profiles, &resolvers);

        Self::load_in_profiles(dir, &active, resolvers)
    }

    /// Loads base and profile files from `dir` without consulting `UPWELL_PROFILES`.
    ///
    /// This is the deterministic counterpart to [`load_in`](Self::load_in) used when a
    /// higher-precedence caller, such as the generated CLI, supplied the complete profile list.
    /// Placeholder substitution retains the default environment resolver.
    pub fn load_in_explicit(dir: &Path, profiles: &[String]) -> Result<Self, ConfigError> {
        Self::load_in_profiles(dir, profiles, ResolverChain::env_default())
    }

    /// Loads one exact base file followed by sibling `<stem>-<profile>.<ext>` overlays.
    ///
    /// The base file is required. Missing profile files are skipped, and the selected file
    /// extension must be supported by this manager's format.
    pub fn load_file(path: &Path, profiles: &[String]) -> Result<Self, ConfigError> {
        let parsers = F::parsers();
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| ConfigError::UnsupportedFormat {
                path: path.to_path_buf(),
            })?;
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .ok_or_else(|| ConfigError::UnsupportedFormat {
                path: path.to_path_buf(),
            })?;
        let parser = parser_for(path, &parsers)?;
        let mut root = ConfigValue::Table(Vec::new());
        let mut sources = Vec::new();

        merge_required_file(&mut root, path, parser)?;
        sources.push(path.to_path_buf());

        for profile in profiles {
            let profile_path = parent.join(format!("{stem}-{profile}.{extension}"));

            if !profile_path.exists() {
                trace!(target: "upwell::config", path = %profile_path.display(), "config profile absent, skipping");

                continue;
            }

            merge_required_file(&mut root, &profile_path, parser)?;
            sources.push(profile_path);
        }

        Ok(Self::wrap(root, sources))
    }

    fn load_in_profiles(
        dir: &Path,
        active: &[String],
        resolvers: ResolverChain,
    ) -> Result<Self, ConfigError> {
        let parsers = F::parsers();

        let mut root = ConfigValue::Table(Vec::new());
        let mut sources = Vec::new();

        debug!(target: "upwell::config", profiles = ?active, "loading config");

        merge_stem(&mut root, dir, "application", &parsers, &mut sources)?;

        for profile in active {
            let stem = format!("application-{profile}");

            merge_stem(&mut root, dir, &stem, &parsers, &mut sources)?;
        }

        info!(target: "upwell::config", sources = sources.len(), profiles = active.len(), "config loaded");

        Ok(Self::wrap_with_resolvers(root, sources, resolvers))
    }

    /// Loads and merges config from `directories`' config directory **with the `${@kind}`
    /// directory namespace registered**, so values can reference `${@runtime}`, `${@data}`,
    /// and friends.
    ///
    /// The one-call equivalent of
    /// [`load_in`](Self::load_in)`(&directories.config_path(), profiles)` followed by
    /// [`with_directories`](Self::with_directories)`(directories)`. Prefer this over
    /// `load_in` whenever a [`DirectoriesManager`] is on hand — `load_in` only receives the
    /// config directory, so it cannot wire the namespace itself.
    pub fn load_from(
        directories: &DirectoriesManager,
        profiles: &[String],
    ) -> Result<Self, ConfigError> {
        Self::load_from_with_resolvers(directories, profiles, ResolverChain::env_default())
    }

    /// Like [`load_from`](Self::load_from), but uses an explicit resolver chain for
    /// profile selection and placeholder substitution before appending the directory
    /// namespace resolver. Profile selection is frozen when the manager is loaded.
    pub fn load_from_with_resolvers(
        directories: &DirectoriesManager,
        profiles: &[String],
        resolvers: ResolverChain,
    ) -> Result<Self, ConfigError> {
        let manager =
            Self::load_in_with_resolvers(&directories.config_path(), profiles, resolvers)?;

        Ok(manager.with_directories(directories))
    }

    fn wrap(root: ConfigValue, sources: Vec<PathBuf>) -> Self {
        Self::wrap_with_resolvers(root, sources, ResolverChain::env_default())
    }

    fn wrap_with_resolvers(
        root: ConfigValue,
        sources: Vec<PathBuf>,
        resolvers: ResolverChain,
    ) -> Self {
        Self {
            root,
            resolvers,
            format: F::ID,
            sources,
            bindings: Vec::new(),
            triggers: ReloadTriggers::default(),
            _marker: PhantomData,
        }
    }
}

impl<F> ConfigManager<F> {
    /// Deserializes the subtree at `path` into `T`, resolving `${...}` placeholders
    /// against the configured resolver chain and other config paths. The single entry
    /// point shared by transport setup in `main` and DI-seeded `Cfg<T>` injection.
    #[instrument(target = "upwell::config", level = "debug", skip(self))]
    pub fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, ConfigError> {
        let subtree = self
            .root
            .get_path(path)
            .ok_or_else(|| ConfigError::MissingPath {
                path: path.to_string(),
            })?;

        // Deserialize the subtree, but resolve placeholders against the full tree so
        // absolute property-path references (`${app.server.port}`) still resolve.
        let value = from_value_in(&self.root, subtree, &self.resolvers).map_err(|source| {
            ConfigError::Substitution {
                path: path.to_string(),
                source,
            }
        })?;

        trace!(target: "upwell::config", "config subtree deserialized");

        Ok(value)
    }

    /// Like [`get`](Self::get), but for a [`ConfigProperties`] type: its `#[default = ".."]`
    /// field defaults are merged *under* the file values before deserializing, so a missing
    /// field falls back to its (possibly templated) default and resolves through the normal
    /// `${...}` pipeline.
    ///
    /// When the `path` subtree is absent and the type declares defaults, deserialization
    /// proceeds from an empty table so a fully-defaulted type still materializes; absent and
    /// default-free remains a [`MissingPath`](ConfigError::MissingPath) error, matching
    /// [`get`](Self::get).
    #[instrument(target = "upwell::config", level = "debug", skip(self))]
    pub fn get_config<T: ConfigProperties>(&self, path: &str) -> Result<T, ConfigError> {
        self.get_config_in::<T>(&self.root, path)
    }

    pub(crate) fn get_config_with_snapshot<T: ConfigProperties>(
        &self,
        path: &str,
    ) -> Result<(T, ConfigSnapshot), ConfigError> {
        self.get_config_in_with_snapshot::<T>(&self.root, path)
    }

    /// [`get_config`](Self::get_config) against an explicit base tree rather than the
    /// manager's current `root`. The reload path uses this to deserialize a binding
    /// from a freshly re-read tree without first adopting it.
    pub(crate) fn get_config_in<T: ConfigProperties>(
        &self,
        base: &ConfigValue,
        path: &str,
    ) -> Result<T, ConfigError> {
        self.get_config_in_with_snapshot::<T>(base, path)
            .map(|(value, _)| value)
    }

    /// Deserializes a binding and captures the placeholder values observed by that
    /// exact pass. A second resolution pass could observe different values from a
    /// mutable resolver, so the value and its dependency fingerprint stay together.
    pub(crate) fn get_config_in_with_snapshot<T: ConfigProperties>(
        &self,
        base: &ConfigValue,
        path: &str,
    ) -> Result<(T, ConfigSnapshot), ConfigError> {
        let defaults = T::DEFAULTS;

        let mut subtree = match base.get_path(path) {
            Some(node) => node.clone(),

            None => {
                if defaults.is_none() {
                    return Err(ConfigError::MissingPath {
                        path: path.to_string(),
                    });
                }

                ConfigValue::Table(Vec::new())
            }
        };

        defaults
            .fill_missing(&mut subtree)
            .map_err(|source| ConfigError::Substitution {
                path: path.to_string(),
                source,
            })?;

        // Resolve against a root that has this type's filled subtree placed at `path`, so a
        // default referencing a sibling (`addr = "${app.server.port}"` where `port` is itself
        // only a default) resolves without relying on the manager having seeded it. Cross-
        // *type* references additionally require those types to be registered (`auto_discover`
        // seeds them into the base tree, which this clone preserves).
        let mut root = base.clone();

        if let Some(node) = ensure_path_mut(&mut root, path) {
            *node = subtree.clone();
        }

        let (value, resolved) =
            crate::de::from_value_in_with_dependencies(&root, &subtree, &self.resolvers).map_err(
                |source| ConfigError::Substitution {
                    path: path.to_string(),
                    source,
                },
            )?;

        trace!(target: "upwell::config", "config subtree deserialized with defaults");

        Ok((
            value,
            ConfigSnapshot {
                raw: base.get_path(path).cloned(),
                resolved,
            },
        ))
    }

    /// Replaces the chain consulted during placeholder substitution.
    pub fn with_resolvers(mut self, resolvers: ResolverChain) -> Self {
        self.resolvers = resolvers;

        self
    }

    /// Appends a [`Resolver`] to the chain consulted during placeholder substitution. Later
    /// resolvers are tried only when earlier resolvers have no value.
    pub fn with_resolver(mut self, resolver: Box<dyn Resolver>) -> Self {
        self.resolvers.0.push(resolver);

        self
    }

    /// Registers the directory namespace, so config values may reference application
    /// directories as `${@runtime}`, `${@config}`, `${@data}`, `${@cache}`, `${@state}`,
    /// and `${@tmp}` (e.g. `socket = "${@runtime}/app.sock"`). Builds a
    /// [`DirectoriesResolver`] from the manager's directory list — config owns the resolver,
    /// dirs just supplies the data.
    pub fn with_directories(self, directories: &DirectoriesManager) -> Self {
        self.with_resolver(Box::new(DirectoriesResolver::from_manager(directories)))
    }

    /// Whether `path` resolves to a present subtree.
    pub fn contains(&self, path: &str) -> bool {
        self.root.get_path(path).is_some()
    }

    /// The format this manager was read in (retained even after erasure, so reload
    /// knows how to re-read).
    pub fn format(&self) -> FormatId {
        self.format
    }

    /// The files this manager was loaded from, in merge order — the inputs a future
    /// reload re-reads.
    pub fn sources(&self) -> &[PathBuf] {
        &self.sources
    }

    /// Erases the `Format` type, keeping the runtime format tag, sources, and bindings. The
    /// DI path stores managers in this form, since the tree is already parsed.
    pub fn into_dynamic(self) -> ConfigManager<Dynamic> {
        ConfigManager {
            root: self.root,
            resolvers: self.resolvers,
            format: self.format,
            sources: self.sources,
            bindings: self.bindings,
            triggers: self.triggers,
            _marker: PhantomData,
        }
    }

    /// Reload the configuration on `SIGHUP` (Unix). Opt-in; manual reload is always
    /// available.
    pub fn reload_on_sighup(mut self) -> Self {
        self.triggers.sighup = true;

        self
    }

    /// Watch the config source files and reload on change. Requires the `watch` feature;
    /// without it the request is logged and ignored at startup.
    pub fn watch_config(mut self) -> Self {
        self.triggers.watch = true;

        self
    }

    /// How long to coalesce a burst of file-change events before reloading (default 250ms).
    pub fn config_reload_debounce(mut self, debounce: Duration) -> Self {
        self.triggers.debounce = debounce;

        self
    }

    /// The automatic reload triggers this manager requests.
    pub fn triggers(&self) -> ReloadTriggers {
        self.triggers
    }

    /// Registers every link-time `#[config(path = "..")]` type (the [`CONFIG_BINDINGS`]
    /// slice), then seeds their defaults into the tree.
    ///
    /// This is where config auto-discovery lives — not the daemon. Call it on the manager
    /// built in `main` so that values read *before* the daemon (transport setup) already see
    /// every type's defaults, which is what lets one type's default reference another type's
    /// path (`${a.b.c}` from a default at `x.y.z`).
    pub fn auto_discover(mut self) -> Self {
        let mut descriptors: Vec<_> = CONFIG_BINDINGS.iter().collect();
        descriptors.sort_by(|left, right| {
            (left.ty.type_name)()
                .cmp((right.ty.type_name)())
                .then_with(|| left.path.cmp(right.path))
        });

        for descriptor in descriptors {
            self.push_binding(descriptor.to_binding());
        }

        self.seed_defaults();

        self
    }

    /// Binds config type `T` to `path` (the explicit, multi-path counterpart to
    /// `#[config(path = "..")]`), then re-seeds defaults. The same type may be bound at
    /// several paths.
    pub fn with_config<T: ConfigProperties>(mut self, path: impl Into<String>) -> Self {
        self.push_binding(ConfigBinding::of::<T>(path));
        self.seed_defaults();

        self
    }

    /// Adds a pre-built binding (used by the daemon to fold in builder-registered configs),
    /// then re-seeds.
    pub fn register_binding(&mut self, binding: ConfigBinding) {
        self.push_binding(binding);
        self.seed_defaults();
    }

    /// Pushes a binding unless an identical one (same type at the same path) is already
    /// registered, so auto-discovering a manager that `main` already auto-discovered (or
    /// re-registering an explicit binding) does not double-bind. Distinct paths of the same
    /// type are kept (the multi-path case).
    fn push_binding(&mut self, binding: ConfigBinding) {
        let duplicate = self.bindings.iter().any(|existing| {
            existing.ty.type_id == binding.ty.type_id && existing.path == binding.path
        });

        if !duplicate {
            self.bindings.push(binding);
        }
    }

    /// The config bindings registered on this manager, in registration order.
    pub fn bindings(&self) -> &[ConfigBinding] {
        &self.bindings
    }

    /// Seeds every bound type's [`DefaultSpec`] into the tree at its path, creating the path
    /// if absent.
    ///
    /// Idempotent: defaults only fill *missing* leaves, so a file value always wins and
    /// re-seeding never clobbers. Seeding parses templates into string leaves without
    /// resolving them, so it is independent of the resolver chain (the `${@dir}` namespace
    /// need not be wired yet). After seeding, a default's `${a.b.c}` reference resolves
    /// because `a.b.c`'s own default is now present in the tree.
    fn seed_defaults(&mut self) {
        seed_defaults_into(&mut self.root, &self.bindings);
    }

    /// Returns whether a candidate tree still matches the exact deserialization pass
    /// that produced a committed binding. With an unchanged raw subtree, only the
    /// placeholders actually consumed by serde are re-resolved.
    pub(crate) fn snapshot_matches(
        &self,
        base: &ConfigValue,
        path: &str,
        committed: &ConfigSnapshot,
    ) -> bool {
        if base.get_path(path) != committed.raw.as_ref() {
            return false;
        }

        let mut ctx = ResolveCtx::new(base, &self.resolvers);

        committed.resolved.iter().all(|dependency| {
            ctx.resolve_placeholder(&dependency.placeholder)
                .is_ok_and(|value| value == dependency.value)
        })
    }

    /// Re-reads every retained source from disk and rebuilds a fresh merged tree
    /// (defaults seeded), **without** adopting it. Rebuilding from empty in the
    /// original merge order preserves precedence: if profile `b` changed, `c` still
    /// overrides `b` and `b` overrides `a`. Returns the new tree for diffing.
    pub(crate) fn reread(&self) -> Result<ConfigValue, ConfigError> {
        let parsers = parsers_for(self.format);

        let mut root = ConfigValue::Table(Vec::new());

        for source in &self.sources {
            merge_file(&mut root, source, &parsers)?;
        }

        seed_defaults_into(&mut root, &self.bindings);

        Ok(root)
    }

    /// Adopts a tree produced by [`reread`](Self::reread) as the current one, after a
    /// reload has committed.
    pub(crate) fn adopt(&mut self, root: ConfigValue) {
        self.root = root;
    }
}

/// The dependency-aware view of one config binding at its last successful commit.
#[derive(Clone, PartialEq)]
pub(crate) struct ConfigSnapshot {
    raw: Option<ConfigValue>,
    resolved: Vec<ResolvedDependency>,
}

/// Seeds every bound type's [`DefaultSpec`] into `root` at its path. See
/// [`ConfigManager::seed_defaults`].
fn seed_defaults_into(root: &mut ConfigValue, bindings: &[ConfigBinding]) {
    for binding in bindings {
        if binding.defaults.is_none() {
            continue;
        }

        let Some(node) = ensure_path_mut(root, &binding.path) else {
            continue;
        };

        if let Err(error) = binding.defaults.fill_missing(node) {
            // Defaults are compile-time literals, so this is unreachable for
            // macro-emitted specs; a hand-built spec with a malformed template lands here
            // and is left unseeded (the real error surfaces at the later typed read).
            debug!(
                target: "upwell::config",
                path = %binding.path,
                %error,
                "skipping unseedable default",
            );
        }
    }
}

/// The `(extension, parser)` pairs for a runtime [`FormatId`] — the reload-time
/// counterpart of [`Format::parsers`], which is only reachable through the erased
/// `Format` type at load.
fn parsers_for(format: FormatId) -> Vec<(&'static str, Parser)> {
    match format {
        FormatId::Toml => Toml::parsers(),
        #[cfg(feature = "yaml")]
        FormatId::Yaml => Yaml::parsers(),
        #[cfg(not(feature = "yaml"))]
        FormatId::Yaml => vec![],
        FormatId::Dynamic => Dynamic::parsers(),
    }
}

/// Merges a single source file into `root`, selecting the parser by the file's
/// extension. A source with no matching parser is skipped.
fn merge_file(
    root: &mut ConfigValue,
    path: &Path,
    parsers: &[(&'static str, Parser)],
) -> Result<(), ConfigError> {
    let extension = path.extension().and_then(|ext| ext.to_str());

    let Some((_, parse)) = parsers.iter().find(|(ext, _)| Some(*ext) == extension) else {
        trace!(target: "upwell::config", path = %path.display(), "no parser for source extension, skipping");

        return Ok(());
    };

    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let parsed = parse(&text).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })?;

    merge_into(root, parsed);

    Ok(())
}

fn parser_for(path: &Path, parsers: &[(&'static str, Parser)]) -> Result<Parser, ConfigError> {
    let extension = path.extension().and_then(|extension| extension.to_str());

    parsers
        .iter()
        .find(|(candidate, _)| Some(*candidate) == extension)
        .map(|(_, parser)| *parser)
        .ok_or_else(|| ConfigError::UnsupportedFormat {
            path: path.to_path_buf(),
        })
}

fn merge_required_file(
    root: &mut ConfigValue,
    path: &Path,
    parser: Parser,
) -> Result<(), ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let parsed = parser(&text).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })?;

    merge_into(root, parsed);

    Ok(())
}

/// Navigates a dotted `path` through `root`, creating an empty table for each missing
/// segment, and returns a mutable reference to the node at that path. Returns `None` if a
/// segment traverses a non-table (a conflicting scalar/array already occupies the path).
fn ensure_path_mut<'a>(root: &'a mut ConfigValue, path: &str) -> Option<&'a mut ConfigValue> {
    let mut current = root;

    for segment in path.split('.') {
        let entries = match current {
            ConfigValue::Table(entries) => entries,
            _ => return None,
        };

        let index = match entries.iter().position(|(key, _)| key == segment) {
            Some(index) => index,

            None => {
                entries.push((segment.to_string(), ConfigValue::Table(Vec::new())));

                entries.len() - 1
            }
        };

        current = &mut entries[index].1;
    }

    Some(current)
}

/// Combines `UPWELL_PROFILES` (consulted first) with the explicitly supplied
/// profiles, preserving order.
fn resolve_profiles(explicit: &[String], resolvers: &ResolverChain) -> Vec<String> {
    let mut profiles = Vec::new();

    if let Some(value) = resolvers.resolve("UPWELL_PROFILES") {
        let from_resolvers = value
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        profiles.extend(from_resolvers);
    }

    profiles.extend(explicit.iter().cloned());

    profiles
}

/// Merges every present `stem.<ext>` file under `dir` into `root`, in parser order.
fn merge_stem(
    root: &mut ConfigValue,
    dir: &Path,
    stem: &str,
    parsers: &[(&'static str, Parser)],
    sources: &mut Vec<PathBuf>,
) -> Result<(), ConfigError> {
    for (ext, parse) in parsers {
        let path = dir.join(format!("{stem}.{ext}"));

        if !path.exists() {
            trace!(target: "upwell::config", path = %path.display(), "config file absent, skipping");

            continue;
        }

        let text = std::fs::read_to_string(&path).map_err(|source| ConfigError::Io {
            path: path.clone(),
            source,
        })?;
        let parsed = parse(&text).map_err(|source| ConfigError::Parse {
            path: path.clone(),
            source,
        })?;

        merge_into(root, parsed);
        sources.push(path.clone());

        debug!(target: "upwell::config", path = %path.display(), "merged config file");
    }

    Ok(())
}

/// Deep-merges `overlay` into `base`: tables recurse key-by-key, every other value
/// (scalar or array) replaces the base value wholesale.
fn merge_into(base: &mut ConfigValue, overlay: ConfigValue) {
    let ConfigValue::Table(overlay_entries) = overlay else {
        *base = overlay;
        return;
    };

    let ConfigValue::Table(base_entries) = base else {
        *base = ConfigValue::Table(overlay_entries);
        return;
    };

    for (key, value) in overlay_entries {
        match base_entries.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => merge_into(&mut slot.1, value),
            None => base_entries.push((key, value)),
        }
    }
}

#[cfg(test)]
mod tests;
