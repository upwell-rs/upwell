use std::collections::HashSet;

use upwell_core::{Singleton as SingletonScope, TypeDescriptor};
use upwell_di::{
    BoxedComponent, Component, ComponentDescriptor, Injectable, RootResolver,
    root_resolver_descriptor,
};
use upwell_dirs::{Cache, Config, Data, Dir, DirKind, DirectoriesManager, Runtime, State, Tmp};
use upwell_hooks::{HOOK_MANAGER_ID, HOOK_MANAGER_NAME, HookDescriptor, HookManager};

use crate::builtins::RuntimeReloader;
use crate::lifecycle::{ShutdownHandle, ShutdownSignal};
use crate::registry::AppRegistry;

/// The framework-provided singleton injectable for triggering graceful shutdown.
pub(crate) static SHUTDOWN_HANDLE_DESCRIPTOR: ComponentDescriptor =
    ComponentDescriptor::of::<ShutdownHandle>().with_identity(
        crate::builtins::shutdown::SHUTDOWN_HANDLE_ID,
        crate::builtins::shutdown::SHUTDOWN_HANDLE_NAME,
    );

/// The framework-provided singleton injectable for requesting a runtime reload.
pub(crate) static RUNTIME_RELOADER_DESCRIPTOR: ComponentDescriptor =
    ComponentDescriptor::of::<RuntimeReloader>().with_identity(
        crate::builtins::reloader::RUNTIME_RELOADER_ID,
        crate::builtins::reloader::RUNTIME_RELOADER_NAME,
    );

/// The framework-provided singleton injectable that runs lifecycle/event hooks.
pub(crate) static HOOK_MANAGER_DESCRIPTOR: ComponentDescriptor = ComponentDescriptor::manual(
    HOOK_MANAGER_ID,
    HOOK_MANAGER_NAME,
    TypeDescriptor::of::<HookManager>(HOOK_MANAGER_NAME),
    &SingletonScope,
);

/// Rejects a factory-less root singleton that nothing seeded before any factory runs.
///
/// The effective graph orders every factory-less singleton as already available, so
/// without this check a factory depending on an unseeded singleton could run, with its
/// side effects, before the root build reached the missing seed.
pub(crate) fn ensure_seeded(
    order: &[ComponentDescriptor],
    instances: &[BoxedComponent],
) -> crate::Result<()> {
    let seeded = instances
        .iter()
        .map(|instance| instance.ty.type_id)
        .collect::<HashSet<_>>();

    for component in order {
        if component.effective_factory()?.is_none() && !seeded.contains(&component.ty.type_id) {
            return Err(upwell_di::Error::MissingComponent(component.name).into());
        }
    }

    Ok(())
}

macro_rules! seed_dirs {
    ($dirs:ident; $registry:ident; $instances:ident; $($name:ident),*) => {
        $(seed_dir::<$name>($dirs, $registry, $instances);)*
    };
}

/// Seeds the [`DirectoriesManager`] and one `Dir<K>` per kind as singleton instances.
pub(crate) fn seed_directories(
    dirs: &DirectoriesManager,
    registry: &mut AppRegistry,
    instances: &mut Vec<BoxedComponent>,
) {
    registry
        .components
        .push(ComponentDescriptor::of::<DirectoriesManager>());
    instances.push(BoxedComponent {
        ty: TypeDescriptor::of::<DirectoriesManager>(<DirectoriesManager as Component>::NAME),
        value: Box::new(dirs.clone()),
    });

    seed_dirs!(
        dirs; registry; instances;
        Config, Data, Cache, State, Runtime, Tmp
    );
}

/// Seeds the framework builtin singletons — the [`ShutdownHandle`] and the [`RootResolver`]
/// (seeded unattached; [`RootResolver::attach`] wires it to the finished root after build).
fn seed_builtins(
    shutdown: &ShutdownSignal,
    root_resolver: &RootResolver,
    registry: &mut AppRegistry,
    instances: &mut Vec<BoxedComponent>,
) {
    registry.components.push(SHUTDOWN_HANDLE_DESCRIPTOR);
    instances.push(BoxedComponent {
        ty: TypeDescriptor::of::<ShutdownHandle>(<ShutdownHandle as Component>::NAME),
        value: Box::new(shutdown.handle()),
    });

    registry.components.push(root_resolver_descriptor());
    instances.push(BoxedComponent {
        ty: TypeDescriptor::of::<RootResolver>(<RootResolver as Component>::NAME),
        value: Box::new(Injectable::into_stored(root_resolver.clone())),
    });
}

/// Seeds one `Dir<K>` as a singleton instance.
fn seed_dir<K: DirKind>(
    dirs: &DirectoriesManager,
    registry: &mut AppRegistry,
    instances: &mut Vec<BoxedComponent>,
) {
    registry
        .components
        .push(ComponentDescriptor::of::<Dir<K>>());
    instances.push(BoxedComponent {
        ty: TypeDescriptor::of::<Dir<K>>(<Dir<K> as Component>::NAME),
        value: Box::new(dirs.dir::<K>()),
    });
}

/// Seeds every framework singleton available before the protocol's pre-build step: the
/// directories, the builtins, and the runtime reloader, plus the hook manager's descriptor,
/// whose instance [`seed_hook_manager`] adds once the collected hooks exist.
pub(super) fn seed_framework(
    dirs: &DirectoriesManager,
    shutdown: &ShutdownSignal,
    root_resolver: &RootResolver,
    runtime_reloader: &RuntimeReloader,
    registry: &mut AppRegistry,
    instances: &mut Vec<BoxedComponent>,
) {
    seed_directories(dirs, registry, instances);
    seed_builtins(shutdown, root_resolver, registry, instances);

    registry.components.push(RUNTIME_RELOADER_DESCRIPTOR);
    instances.push(BoxedComponent {
        ty: TypeDescriptor::of::<RuntimeReloader>(crate::builtins::reloader::RUNTIME_RELOADER_NAME),
        value: Box::new(Injectable::into_stored(runtime_reloader.clone())),
    });
    registry.components.push(HOOK_MANAGER_DESCRIPTOR);
}

/// Seeds the hook manager holding every resolved component's `#[hook]` methods.
pub(super) fn seed_hook_manager(
    resolved: &[ComponentDescriptor],
    instances: &mut Vec<BoxedComponent>,
) {
    let hooks: Vec<HookDescriptor> = resolved
        .iter()
        .flat_map(|component| (component.hooks)().iter().copied())
        .collect();

    instances.push(BoxedComponent {
        ty: TypeDescriptor::of::<HookManager>(HOOK_MANAGER_NAME),
        value: Box::new(Injectable::into_stored(HookManager::new(hooks))),
    });
}
