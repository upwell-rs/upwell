use std::any::TypeId;
use std::collections::HashSet;

use tracing::error;
use upwell_core::TypeDescriptor;
use upwell_hooks::{HookKind, Shutdown, Startup};

use crate::runtime::AppRuntime;

/// Fires the startup event, returning the components whose startup gates all succeeded.
///
/// Gates (participants) run sequentially and the first failure aborts startup; the
/// returned set lets the caller pair shutdown only with work that actually started.
/// Observers run once every gate passed; their failures are logged and never abort.
#[allow(clippy::result_large_err)]
pub(super) async fn run_startup(
    runtime: &AppRuntime,
) -> Result<HashSet<TypeId>, (crate::Error, HashSet<TypeId>)> {
    let view = runtime.view();
    let fired = view.hooks().fire_all::<Startup>(&()).await;
    let mut started = HashSet::new();

    for (component, result) in fired.participants {
        let component_ty = component.type_id;

        match result {
            Ok(()) => {
                started.insert(component_ty);
            }
            Err(error) => {
                error!(
                    target: "upwell::app",
                    hook = Startup::NAME,
                    component = %component.name,
                    %error,
                    "lifecycle hook failed"
                );

                return Err((error.into(), started));
            }
        }
    }

    log_observer_failures::<Startup>(fired.observers);

    Ok(started)
}

/// Fires the observer-only shutdown event for components with no startup gate and for
/// components whose startup gates all succeeded. Errors are logged and cleanup continues.
///
/// It fires on the generation current when shutdown begins, so the live instances receive
/// it rather than the ones a reload has since retired. A direct reload committing from
/// another task while it runs is not coordinated: the observers keep addressing the
/// generation pinned here. `started` holds component types, so a component a reload
/// replaced still pairs with its predecessor's startup, while one a reload added with an
/// unrun startup gate is skipped. `always`-routed hooks bypass that pairing and run
/// regardless.
pub(super) async fn run_shutdown(runtime: &AppRuntime, started: &HashSet<TypeId>) {
    let view = runtime.view();
    let hooks = view.hooks();
    let fired = hooks
        .fire::<Shutdown>(&(), |hook| {
            let component_ty = hook.component_ty.type_id;

            !hooks.component_participates::<Startup>(component_ty)
                || started.contains(&component_ty)
        })
        .await;

    log_observer_failures::<Shutdown>(fired.observers);
}

/// Logs every failed lifecycle observer. Observer failures never change the lifecycle.
fn log_observer_failures<K: HookKind>(observers: Vec<(TypeDescriptor, upwell_hooks::Result<()>)>) {
    for (component, result) in observers {
        if let Err(error) = result {
            error!(
                target: "upwell::app",
                hook = K::NAME,
                component = %component.name,
                %error,
                "lifecycle hook failed"
            );
        }
    }
}
