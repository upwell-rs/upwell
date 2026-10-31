//! A per-call component with [`FAN_IN`] dependencies, built through the ordinary `Factory` path
//! that a synchronous `#[init]` constructor uses, so a bench measures how a factory reads its
//! dependency edges.

use std::sync::Arc;

use upwell_core::ScopeKey;
use upwell_di::{
    Component, ComponentDescriptor, ComponentFactoryDescriptor, ComponentFactoryRole,
    dispatch_factory, dispatch_ready_factory, factory_dependencies,
};

use crate::scoped::CallScoped;

/// The number of roster components [`FanIn`] depends on.
pub const FAN_IN: usize = 8;

/// A per-call component depending on the first [`FAN_IN`] roster components.
pub struct FanIn;

impl Component for FanIn {
    type Handle = Arc<Self>;

    const ID: &'static str = "fan-in-component";
    const NAME: &'static str = "FanIn";

    fn into_handle(self) -> Arc<Self> {
        Arc::new(self)
    }
}

/// Declares a constructor `$name` taking the [`FAN_IN`] roster dependencies, sync or `async`.
macro_rules! fan_in_constructor {
    ($name:ident $($async:ident)?) => {
        #[allow(clippy::too_many_arguments)]
        $($async)? fn $name(
            _: Arc<CallScoped<0>>,
            _: Arc<CallScoped<1>>,
            _: Arc<CallScoped<2>>,
            _: Arc<CallScoped<3>>,
            _: Arc<CallScoped<4>>,
            _: Arc<CallScoped<5>>,
            _: Arc<CallScoped<6>>,
            _: Arc<CallScoped<7>>,
        ) -> FanIn {
            FanIn
        }
    };
}

fan_in_constructor!(fan_in);
fan_in_constructor!(init async);

fn factories() -> &'static [ComponentFactoryDescriptor] {
    const {
        &[ComponentFactoryDescriptor {
            id: "explicit",
            construct: |cx| dispatch_factory(init, cx),
            construct_ready: Some(|cx| dispatch_ready_factory(fan_in, cx)),
            dependencies: || factory_dependencies(init),
            role: ComponentFactoryRole::Explicit,
        }]
    }
}

/// The [`FanIn`] descriptor in `scope`, leaked to `'static` as the app builder requires.
pub fn descriptor(scope: ScopeKey) -> &'static ComponentDescriptor {
    Box::leak(Box::new(ComponentDescriptor {
        scope,
        factories,
        ..ComponentDescriptor::of::<FanIn>()
    }))
}
