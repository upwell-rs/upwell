use upwell_config::{
    ConditionFactSource, ConditionFacts, ConfigBinding, ConfigManager, ConfigProperties,
};
use upwell_core::{Descriptor, EntryKey, Scope, Singleton as SingletonScope, TypeDescriptor};
use upwell_di::{BoxedComponent, Component, ComponentDescriptor, Injectable, ProviderDescriptor};
use upwell_dirs::DirectoriesManager;

use super::App;
use crate::entry::{EntryDescriptor, EntryTargets};
use crate::host::ExecutionMode;
use crate::plugin::{EarlyPluginCatalog, Plugin, PluginCatalog, PluginWithOptions};
use crate::protocol::ProtocolDefinition;
use crate::registry::AppRegistry;

/// Assembles an [`App`] from an explicit set of components and a protocol definition.
///
/// Generic over the [`ProtocolDefinition`] it prepares. The agnostic builder methods (config,
/// components, directories, auto-discovery) live here; protocol-specific methods come from
/// an extension trait (e.g. `RpcAppBuilder` in `upwell-rpc`), so the same builder serves
/// any protocol.
pub struct AppBuilder<D: ProtocolDefinition> {
    pub(super) name: String,
    pub(super) registry: AppRegistry,
    pub(super) instances: Vec<BoxedComponent>,
    pub(super) config_source: Option<ConfigManager>,
    /// Whether link-time application, protocol, config, and plugin discovery is enabled.
    pub(super) auto_discovery_enabled: bool,
    pub(super) dirs: Option<DirectoriesManager>,
    pub(super) plugins: PluginCatalog,
    /// The selected protocol definition and its accumulated configuration.
    pub(super) protocol: D,
    /// Tooling preparation evaluates only conditions it can decide without trusted code.
    pub(super) mode: ExecutionMode,
}

impl<D: ProtocolDefinition> AppBuilder<D> {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            registry: AppRegistry::default(),
            instances: Vec::new(),
            config_source: None,
            auto_discovery_enabled: false,
            dirs: None,
            plugins: PluginCatalog::new(),
            protocol: D::default(),
            mode: ExecutionMode::Run,
        }
    }

    /// Prepares for `mode`: tooling preparation never runs trusted condition callbacks, and
    /// cannot be built while it leaves a condition undecided.
    pub(crate) fn execution_mode(mut self, mode: ExecutionMode) -> Self {
        self.mode = mode;

        self
    }

    /// Mutable access to the protocol definition, for protocol-specific extension traits.
    pub fn protocol_mut(&mut self) -> &mut D {
        &mut self.protocol
    }

    /// Mutable access to the agnostic registry, for protocol-specific extension traits
    /// that also register components (e.g. a service's component descriptor).
    pub fn registry_mut(&mut self) -> &mut AppRegistry {
        &mut self.registry
    }

    /// Supplies the merged configuration the app binds its `Cfg<T>` injections from. If
    /// omitted, the app loads config from its `Dir<Config>` directory.
    pub fn config_source<F>(mut self, config: ConfigManager<F>) -> Self {
        self.config_source = Some(config.into_dynamic());

        self
    }

    /// Supplies the [`DirectoriesManager`] the app seeds its `Dir<K>` injectables from.
    pub fn directories(mut self, dirs: DirectoriesManager) -> Self {
        self.dirs = Some(dirs);

        self
    }

    /// Binds config type `T` to the subtree at `path`, injectable as `Cfg<T>`.
    pub fn config<T: ConfigProperties>(mut self, path: impl Into<String>) -> Self {
        self.registry
            .config_bindings
            .push(ConfigBinding::of::<T>(path));

        self
    }

    /// Registers the condition facts of config type `T` at `path`, so conditional
    /// components can key their availability on `T`'s values. The path is part of every
    /// fact id the source emits, so the same type may be registered at several paths.
    /// The transactional reload extracts the scalars from every staged binding of the
    /// type.
    pub fn condition_facts<T: ConditionFacts>(mut self, path: &'static str) -> Self {
        self.registry
            .condition_facts
            .push(ConditionFactSource::of::<T>(path));

        self
    }

    /// Registers a pre-built singleton instance, holding it until the container is built.
    ///
    /// The instance is seeded with a raw manual descriptor: it carries no
    /// generation-snapshot adapter, so a runtime transition that would retain it
    /// requires a process restart instead of sharing the instance.
    pub fn with_component<T: Component>(mut self, value: T) -> Self {
        self.registry.components.push(ComponentDescriptor::manual(
            T::ID,
            T::NAME,
            TypeDescriptor::of::<T>(T::NAME),
            SingletonScope::KEY,
        ));

        self.instances.push(BoxedComponent {
            ty: TypeDescriptor::of::<T>(T::NAME),
            value: Box::new(Injectable::into_stored(value.into_handle())),
        });

        self
    }

    /// Merges every link-time-registered component, provider and `#[entry]` descriptor (and
    /// the protocol's own variants) into the app, and enables config auto-discovery.
    pub fn auto_discover(mut self) -> Self {
        let discovered = AppRegistry::collect();

        self.registry.components.extend(discovered.components);

        for provider in discovered.providers {
            self.registry.add_provider(provider);
        }

        for entry in discovered.entries {
            self.registry.add_entry(entry);
        }

        self.auto_discovery_enabled = true;
        self.protocol.auto_discover();

        self
    }

    /// Registers component type `T` for construction from its statically-known descriptor.
    pub fn component<T>(mut self) -> Self
    where
        T: Descriptor<ComponentDescriptor>,
    {
        self.registry
            .components
            .push(<T as Descriptor<ComponentDescriptor>>::DESCRIPTOR);

        self
    }

    /// Manually register a raw component descriptor for construction during build.
    pub fn component_descriptor(mut self, descriptor: &'static ComponentDescriptor) -> Self {
        self.registry.components.push(*descriptor);

        self
    }

    /// Registers every trait mapping component type `T` declares with
    /// `provide = dyn Trait`. Pair it with [`component`](Self::component) when
    /// registering a providing component explicitly instead of through discovery; a
    /// mapping that discovery also registers is kept once.
    pub fn provider<T>(mut self) -> Self
    where
        T: Descriptor<&'static [ProviderDescriptor]>,
    {
        for provider in <T as Descriptor<&'static [ProviderDescriptor]>>::DESCRIPTOR {
            self.registry.add_provider(*provider);
        }

        self
    }

    /// Registers a raw provider mapping, such as one built with
    /// [`ProviderDescriptor::of`].
    pub fn provider_descriptor(mut self, descriptor: &'static ProviderDescriptor) -> Self {
        self.registry.add_provider(*descriptor);

        self
    }

    /// Registers an entry into scope `S` that injects `T`, a tuple of injectable types, under
    /// the stable identity `key`.
    ///
    /// Preparation fails when `S` is not opened by the active protocol, or when a target
    /// cannot be resolved from `S`.
    pub fn scope_entry<S: Scope, T: EntryTargets>(self, key: EntryKey) -> Self {
        self.entry_descriptor(EntryDescriptor::new::<S, T>(key))
    }

    /// Registers the entry `E` declares, such as the marker type an `#[entry]` function
    /// generates. An entry that discovery also registers is kept once.
    pub fn entry<E>(self) -> Self
    where
        E: Descriptor<EntryDescriptor>,
    {
        self.entry_descriptor(<E as Descriptor<EntryDescriptor>>::DESCRIPTOR)
    }

    /// Registers an entry descriptor, for entries assembled without a typed target tuple.
    pub fn entry_descriptor(mut self, descriptor: EntryDescriptor) -> Self {
        self.registry.add_entry(descriptor);

        self
    }

    /// Retains plugin type `P` for deterministic composition during preparation.
    pub fn register_plugin<P: Plugin + Default>(mut self) -> Self {
        self.plugins.with_plugin(P::default());

        self
    }

    /// Retains plugin type `P` constructed synchronously from explicit options.
    pub fn register_plugin_with_options<P: PluginWithOptions>(
        mut self,
        options: P::Options,
    ) -> Self {
        self.plugins.with_plugin(P::from_options(options));

        self
    }

    /// Retains a supplied plugin instance for deterministic composition during preparation.
    pub fn with_plugin<P: Plugin>(mut self, plugin: P) -> Self {
        self.plugins.with_plugin(plugin);

        self
    }

    pub(crate) fn with_early_plugin_catalog(mut self, catalog: EarlyPluginCatalog) -> Self {
        self.plugins.use_early(catalog);

        self
    }

    /// Applies parser-visible static plugin declarations to a directly prepared builder.
    #[doc(hidden)]
    pub fn with_plugin_declarations(
        mut self,
        declarations: impl FnOnce(&mut crate::ApplicationPluginRegistrar),
    ) -> Self {
        self.plugins.declare(declarations);

        self
    }

    /// Validates, constructs, and finalizes a ready-to-run [`App`].
    pub async fn build(self) -> Result<App<D>, D::Error> {
        self.prepare()?.build().await
    }
}
