//! Fault-injection components for the hardening matrix: a sentinel whose `always`
//! config-reload hook rejects or panics on demand, a consumer whose dependency nobody
//! provides once `auth.mode = "orphan"`, and a second primary provider eligible once
//! `auth.mode = "ambiguous"`. Tests sharing these statics hold the fixture's test guard.

use std::any::Any;
use std::sync::Mutex;

use upwell_config::{ConfigReload, HookOutcome};
use upwell_core::{Cardinality, ConditionDescriptor, DependencyDescriptor, TypeDescriptor};
use upwell_di::{Component, ComponentDescriptor, ProviderDescriptor, factories};
use upwell_hooks::{HookDescriptor, HookRouting};

use super::super::fixture::{AUTH_MODE, Authenticator};
use crate::test_support::{hook_call, test_component, unbuilt_factories};

/// What the sentinel's config-reload hook does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ConfigHookFault {
    Accept,
    Reject,
    Panic,
}

static CONFIG_HOOK_FAULT: Mutex<ConfigHookFault> = Mutex::new(ConfigHookFault::Accept);

pub(super) fn set_config_hook_fault(fault: ConfigHookFault) {
    *CONFIG_HOOK_FAULT.lock().expect("config hook fault") = fault;
}

/// A singleton owning an `always` config-reload hook.
pub(super) struct Sentinel;

test_component!(Sentinel, "hardening-sentinel");

async fn construct_sentinel() -> Sentinel {
    Sentinel
}

hook_call!(sentinel_reload_call, |_ctx, _cx| {
    let fault = *CONFIG_HOOK_FAULT.lock().expect("config hook fault");

    async move {
        match fault {
            ConfigHookFault::Accept => Ok(Box::new(HookOutcome::Unchanged) as Box<dyn Any + Send>),
            ConfigHookFault::Reject => Err(upwell_hooks::Error::Other(
                "the sentinel rejects every proposal".into(),
            )),
            ConfigHookFault::Panic => panic!("the sentinel config hook panicked"),
        }
    }
});

const SENTINEL_TY: TypeDescriptor = TypeDescriptor::of::<Sentinel>(Sentinel::NAME);

static SENTINEL_HOOKS: [HookDescriptor; 1] =
    [
        HookDescriptor::of::<ConfigReload>(1, SENTINEL_TY, sentinel_reload_call)
            .with_routing(HookRouting::Always),
    ];

pub(super) static SENTINEL_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<Sentinel>()
    .with_factories(factories!["static" => construct_sentinel])
    .with_hooks(|| &SENTINEL_HOOKS)
    .without_retention();

/// A type no component provides.
pub(super) struct Unprovided;

/// Eligible only while `auth.mode = "orphan"`, and then unsatisfiable.
pub(super) struct Orphan;

test_component!(Orphan, "hardening-orphan");

fn orphan_dependencies() -> Vec<DependencyDescriptor> {
    vec![upwell_di::dependency_of::<Unprovided>(
        Cardinality::One,
        false,
        false,
    )]
}

static ORPHAN_CONDITION: ConditionDescriptor =
    ConditionDescriptor::config_equals("auth-mode-orphan", AUTH_MODE, "orphan");

pub(super) static ORPHAN_COMPONENT: ComponentDescriptor = ComponentDescriptor::of::<Orphan>()
    .with_condition(&ORPHAN_CONDITION)
    .with_factories(unbuilt_factories!("static", orphan_dependencies, Default));

/// A second primary `Authenticator`, eligible only while `auth.mode = "ambiguous"`, when the
/// fallback's primary mapping is eligible too.
pub(super) struct RivalAuthenticator;

impl Authenticator for RivalAuthenticator {
    fn scheme(&self) -> &'static str {
        "rival"
    }
}

test_component!(RivalAuthenticator, "hardening-rival-auth", provides dyn Authenticator);

static RIVAL_CONDITION: ConditionDescriptor =
    ConditionDescriptor::config_equals("auth-mode-ambiguous", AUTH_MODE, "ambiguous");

pub(super) static RIVAL_COMPONENT: ComponentDescriptor =
    ComponentDescriptor::of::<RivalAuthenticator>()
        .with_condition(&RIVAL_CONDITION)
        .with_factories(unbuilt_factories!("static", Vec::new, Default));

pub(super) static RIVAL_PROVIDER: ProviderDescriptor =
    ProviderDescriptor::of::<RivalAuthenticator, dyn Authenticator>("dyn Authenticator").primary();
