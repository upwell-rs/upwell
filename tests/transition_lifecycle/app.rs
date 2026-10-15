//! The `Authenticator` application the transition lifecycle test drives: a default and a
//! custom provider whose availability keys on opposite values of `auth.mode`, a fixed
//! [`AuthGate`] consumer, a [`ChangeFreeze`] operator policy, an [`AuthWatcher`]
//! config-reload hook owner, and a [`TransitionAudit`] observer. Every component is an
//! ordinary `#[component]` whose hooks are `#[hook(..)]` methods; only the two providers'
//! conditions and trait mappings are attached through explicit descriptors, until the
//! condition builder and attribute APIs land (#296, #297).
//!
//! Each hook appends what it saw to [`events`]. The statics are per process, and each test
//! holds [`lock`] so the default `cargo test` threading cannot interleave them.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use tempfile::TempDir;
use upwell::config::Toml;
use upwell::{
    App, AppError, BoxedComponent, Cfg, CfgNext, Component, ComponentDescriptor, ComponentRetired,
    ComponentRetirement, ConditionDescriptor, ConditionFacts, ConditionPredicate, ConditionScalar,
    ConditionScalarKind, ConditionScalarLiteral, ConfigFactDescriptor, ConfigFactId, ConfigManager,
    ConfigReload, Descriptor, HookOutcome, Injectable, Live, NodeAction, PendingTransition,
    ProviderDescriptor, RetirementReason, RuntimeTransitionReport, Shutdown, TransitionAborted,
    TransitionCommitted, TransitionDecision, TransitionProposal, TransitionVerdict, TypeDescriptor,
    component, config, injectable, methods,
};
use upwell_config::ResolverChain;

/// The trait both providers implement and the gate consumes.
#[injectable]
pub trait Authenticator: Send + Sync {
    fn scheme(&self) -> &'static str;
}

#[config(path = "auth")]
#[derive(Deserialize)]
pub struct AuthConfig {
    pub mode: String,
}

const AUTH_MODE: ConfigFactId = ConfigFactId::new("AuthConfig", "auth", "mode");

impl ConditionFacts for AuthConfig {
    fn condition_facts(binding_path: &'static str) -> Vec<ConfigFactDescriptor> {
        vec![ConfigFactDescriptor {
            id: ConfigFactId::new("AuthConfig", binding_path, "mode"),
            kind: ConditionScalarKind::String,
            source: upwell::descriptor_source!(),
        }]
    }

    fn condition_scalars(
        &self,
        binding_path: &'static str,
    ) -> Vec<(ConfigFactId, ConditionScalar)> {
        vec![(
            ConfigFactId::new("AuthConfig", binding_path, "mode"),
            ConditionScalar::string(self.mode.clone()),
        )]
    }
}

/// What one hook observed.
#[derive(Clone, Debug)]
pub enum Event {
    /// The gate inspected a pending transition before construction.
    Proposal {
        components: Vec<(&'static str, NodeAction)>,
        scheme: &'static str,
    },
    /// The change-freeze policy inspected the constructed candidate.
    Decision {
        components: Vec<(&'static str, NodeAction)>,
        config_hooks: usize,
    },
    /// The watcher's config-reload hook ran against the staged mode.
    ConfigReload { mode: String },
    /// The audit observed a committed attempt.
    Committed(RuntimeTransitionReport),
    /// The audit observed an aborted attempt.
    Aborted(RuntimeTransitionReport),
    /// A component's shutdown hook ran; `scheme` is what the receiver serves.
    Shutdown {
        component: &'static str,
        scheme: &'static str,
    },
    /// A displaced component retired; `scheme` is what the retired receiver serves.
    Retired {
        retirement: ComponentRetirement,
        scheme: &'static str,
    },
}

static EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());
static FREEZE: Mutex<Option<&'static str>> = Mutex::new(None);
static GUARD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub async fn lock() -> tokio::sync::MutexGuard<'static, ()> {
    GUARD.lock().await
}

fn record(event: Event) {
    EVENTS.lock().expect("events").push(event);
}

/// Drains every event recorded since the last call.
pub fn events() -> Vec<Event> {
    std::mem::take(&mut *EVENTS.lock().expect("events"))
}

/// Makes the change-freeze policy reject every decision with `reason`, or accept again.
pub fn freeze(reason: Option<&'static str>) {
    *FREEZE.lock().expect("freeze") = reason;
}

fn actions(pending: &PendingTransition) -> Vec<(&'static str, NodeAction)> {
    pending
        .components()
        .iter()
        .map(|component| (component.component, component.action))
        .collect()
}

/// The provider used when `auth.mode` is anything but `"custom"`.
#[component]
pub struct DefaultAuthenticator;

impl Authenticator for DefaultAuthenticator {
    fn scheme(&self) -> &'static str {
        "default"
    }
}

#[methods]
impl DefaultAuthenticator {
    #[hook(ComponentRetired)]
    async fn retired(&self, retirement: ComponentRetirement) {
        record(Event::Retired {
            retirement,
            scheme: self.scheme(),
        });
    }

    #[hook(Shutdown)]
    async fn shutdown(&self) {
        record(Event::Shutdown {
            component: Self::ID,
            scheme: self.scheme(),
        });
    }
}

/// The application's own provider, enabled by `auth.mode = "custom"`.
#[component]
pub struct CustomAuthenticator;

impl Authenticator for CustomAuthenticator {
    fn scheme(&self) -> &'static str {
        "custom"
    }
}

#[methods]
impl CustomAuthenticator {
    #[hook(ComponentRetired)]
    async fn retired(&self, retirement: ComponentRetirement) {
        record(Event::Retired {
            retirement,
            scheme: self.scheme(),
        });
    }

    #[hook(Shutdown)]
    async fn shutdown(&self) {
        record(Event::Shutdown {
            component: Self::ID,
            scheme: self.scheme(),
        });
    }
}

/// A fixed consumer that captured the effective provider at construction.
#[component]
pub struct AuthGate {
    authenticator: Arc<dyn Authenticator>,
}

impl AuthGate {
    pub fn scheme(&self) -> &'static str {
        self.authenticator.scheme()
    }
}

#[methods]
impl AuthGate {
    #[hook(TransitionProposal)]
    async fn propose(&self, pending: PendingTransition) -> TransitionVerdict {
        record(Event::Proposal {
            components: actions(&pending),
            scheme: self.scheme(),
        });

        TransitionVerdict::Accept
    }

    #[hook(ComponentRetired)]
    async fn retired(&self, retirement: ComponentRetirement) {
        record(Event::Retired {
            retirement,
            scheme: self.scheme(),
        });
    }

    #[hook(Shutdown)]
    async fn shutdown(&self) {
        record(Event::Shutdown {
            component: Self::ID,
            scheme: self.scheme(),
        });
    }
}

/// An operator policy no switch affects, gating every decision.
#[component]
pub struct ChangeFreeze;

#[methods]
impl ChangeFreeze {
    #[hook(TransitionDecision, always)]
    async fn decide(&self, pending: PendingTransition) -> TransitionVerdict {
        let frozen = *FREEZE.lock().expect("freeze");

        record(Event::Decision {
            components: actions(&pending),
            config_hooks: pending.config_hooks().len(),
        });

        frozen.map_or(TransitionVerdict::Accept, TransitionVerdict::Reject)
    }
}

/// Owns the `auth` config-reload hook, so every switch reconstructs it.
#[component]
pub struct AuthWatcher {
    #[config("auth")]
    auth: Cfg<AuthConfig>,
}

#[methods]
impl AuthWatcher {
    #[hook(ConfigReload)]
    async fn on_reload(
        &self,
        #[config("auth")] next: CfgNext<AuthConfig>,
    ) -> Result<HookOutcome, AppError> {
        record(Event::ConfigReload {
            mode: next.mode.clone(),
        });

        Ok(HookOutcome::Reloaded)
    }
}

/// Observes every transition outcome.
#[component]
pub struct TransitionAudit;

#[methods]
impl TransitionAudit {
    #[hook(TransitionCommitted)]
    async fn committed(&self, report: RuntimeTransitionReport) {
        record(Event::Committed(report));
    }

    #[hook(TransitionAborted)]
    async fn aborted(&self, report: RuntimeTransitionReport) {
        record(Event::Aborted(report));
    }
}

static CUSTOM_ON: ConditionDescriptor = ConditionDescriptor {
    id: "auth-mode-custom",
    source: upwell::descriptor_source!(),
    predicate: ConditionPredicate::ConfigEquals {
        fact: AUTH_MODE,
        expected: ConditionScalarLiteral::String("custom"),
    },
};

static CUSTOM_OFF: ConditionDescriptor = ConditionDescriptor {
    id: "auth-mode-default",
    source: upwell::descriptor_source!(),
    predicate: ConditionPredicate::Not(&CUSTOM_ON),
};

static DEFAULT_PROVIDER: ComponentDescriptor = ComponentDescriptor {
    condition: Some(&CUSTOM_OFF),
    ..<DefaultAuthenticator as Descriptor<ComponentDescriptor>>::DESCRIPTOR
};

static CUSTOM_PROVIDER: ComponentDescriptor = ComponentDescriptor {
    condition: Some(&CUSTOM_ON),
    ..<CustomAuthenticator as Descriptor<ComponentDescriptor>>::DESCRIPTOR
};

/// Re-erases a built provider under the `dyn Authenticator` key.
fn erase<T: Authenticator + 'static>(boxed: &BoxedComponent) -> BoxedComponent {
    let live = boxed
        .value
        .downcast_ref::<Live<T>>()
        .expect("a provider slot stores its own live handle");
    let erased: Arc<dyn Authenticator> = live.snapshot();

    BoxedComponent {
        ty: TypeDescriptor::of::<dyn Authenticator>("dyn Authenticator"),
        value: Box::new(Injectable::into_stored(erased)),
    }
}

const fn provider<T: Authenticator + 'static>(
    name: &'static str,
    qualifier: &'static str,
) -> ProviderDescriptor {
    ProviderDescriptor {
        trait_ty: TypeDescriptor::of::<dyn Authenticator>("dyn Authenticator"),
        concrete_ty: TypeDescriptor::of::<T>(name),
        qualifier,
        primary: true,
        priority: 0,
        ordering: &[],
        erase: erase::<T>,
    }
}

pub fn write_mode(config_dir: &Path, mode: &str) {
    fs::write(
        config_dir.join("application.toml"),
        format!("[auth]\nmode = \"{mode}\"\n"),
    )
    .expect("write auth config");
}

/// Builds the app over a fresh config directory with `auth.mode = mode`.
pub async fn build(mode: &str) -> (TempDir, PathBuf, App<()>) {
    let dir = tempfile::Builder::new()
        .prefix("upwell-transition-lifecycle-")
        .tempdir()
        .expect("create temp dir");
    let config_dir = dir.path().join("config");

    fs::create_dir_all(&config_dir).expect("create config dir");
    write_mode(&config_dir, mode);

    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(&config_dir, &[], ResolverChain::empty())
            .expect("load config");
    let mut builder = App::<()>::builder("transition-lifecycle-test")
        .config_source(manager)
        .config::<AuthConfig>("auth")
        .condition_facts::<AuthConfig>("auth")
        .component_descriptor(&DEFAULT_PROVIDER)
        .component_descriptor(&CUSTOM_PROVIDER)
        .component::<AuthGate>()
        .component::<ChangeFreeze>()
        .component::<AuthWatcher>()
        .component::<TransitionAudit>();

    builder.registry_mut().providers.extend([
        provider::<DefaultAuthenticator>("DefaultAuthenticator", "default"),
        provider::<CustomAuthenticator>("CustomAuthenticator", "custom"),
    ]);

    let app = builder.build().await.expect("the auth app builds");

    freeze(None);
    events();

    (dir, config_dir, app)
}

/// The retirement a displaced component received, by its stable ID.
pub fn retirement_of(events: &[Event], component: &str) -> Vec<(RetirementReason, &'static str)> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Retired { retirement, scheme } if retirement.component == component => {
                Some((retirement.reason, *scheme))
            }
            _ => None,
        })
        .collect()
}
