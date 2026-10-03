//! Fixture for the Authenticator provider-switch acceptance test: a file-backed app
//! with two condition-gated `Authenticator` provider components whose availability
//! keys on opposite sides of one `auth.mode` config fact, so exactly one provider is
//! eligible at any time.
//!
//! The components are declared with explicit builder descriptors and unique ids — no
//! `#[component]` macros — so nothing registers into the link-time inventory and the
//! test cannot interfere with discovery-based tests. Each factory injects
//! `Cfg<AuthConfig>` and records the config mode its construction resolved, so the
//! test can prove which store each stage of the transaction read. The fixture owns
//! its statics alone and shares nothing with the probe fixture, so it needs no
//! cross-test guard.

use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use serde::Deserialize;
use tempfile::TempDir;
use upwell_config::{Cfg, ConditionFacts, ConfigManager, ConfigProperties, ResolverChain, Toml};
use upwell_core::{
    ConditionDescriptor, ConditionPredicate, ConditionScalar, ConditionScalarKind,
    ConditionScalarLiteral, ConfigFactDescriptor, ConfigFactId, DependencyDescriptor,
    TypeDescriptor,
};
use upwell_di::{
    BoxedComponent, Component, ComponentConstructionContext, ComponentDescriptor,
    ComponentFactoryDescriptor, FromContainer, Injectable, Live, ProviderDescriptor, Singleton,
};

use crate::App;

/// The trait both provider components implement and the dependency side resolves as
/// `Arc<dyn Authenticator>`.
pub(super) trait Authenticator: Send + Sync {
    fn scheme(&self) -> &'static str;
}

const AUTH_MODE: ConfigFactId = ConfigFactId::new("AuthConfig", "auth", "mode");

/// The primary is eligible exactly when `auth.mode` is `"primary"`.
static PRIMARY_CONDITION: ConditionDescriptor = ConditionDescriptor {
    id: "auth-mode-primary",
    source: upwell_core::descriptor_source!(),
    predicate: ConditionPredicate::ConfigEquals {
        fact: AUTH_MODE,
        expected: ConditionScalarLiteral::String("primary"),
    },
};

/// The fallback is eligible exactly when the primary is not, so the two conditions
/// are mutually exclusive by construction.
static FALLBACK_CONDITION: ConditionDescriptor = ConditionDescriptor {
    id: "auth-mode-fallback",
    source: upwell_core::descriptor_source!(),
    predicate: ConditionPredicate::Not(&PRIMARY_CONDITION),
};

/// The config mode each factory construction resolved, in construction order.
static OBSERVED_MODES: tokio::sync::Mutex<Vec<String>> = tokio::sync::Mutex::const_new(Vec::new());

pub(super) async fn observed_modes() -> Vec<String> {
    OBSERVED_MODES.lock().await.clone()
}

pub(super) async fn clear_observed_modes() {
    OBSERVED_MODES.lock().await.clear();
}

#[derive(Deserialize)]
struct AuthConfig {
    mode: String,
}

impl ConfigProperties for AuthConfig {
    const NAME: &'static str = "AuthConfig";
}

impl ConditionFacts for AuthConfig {
    fn condition_facts() -> Vec<ConfigFactDescriptor> {
        vec![ConfigFactDescriptor {
            id: AUTH_MODE,
            kind: ConditionScalarKind::String,
            source: upwell_core::descriptor_source!(),
        }]
    }

    fn condition_scalars(&self) -> Vec<(ConfigFactId, ConditionScalar)> {
        vec![(AUTH_MODE, ConditionScalar::string(self.mode.clone()))]
    }
}

pub(super) struct PrimaryAuthenticator {
    pub(super) observed_mode: String,
}

impl Authenticator for PrimaryAuthenticator {
    fn scheme(&self) -> &'static str {
        "primary"
    }
}

impl Component for PrimaryAuthenticator {
    type Handle = Arc<Self>;

    const ID: &'static str = "auth-primary";
    const NAME: &'static str = "PrimaryAuthenticator";

    fn into_handle(self) -> Self::Handle {
        Arc::new(self)
    }
}

pub(super) struct FallbackAuthenticator {
    pub(super) observed_mode: String,
}

impl Authenticator for FallbackAuthenticator {
    fn scheme(&self) -> &'static str {
        "fallback"
    }
}

impl Component for FallbackAuthenticator {
    type Handle = Arc<Self>;

    const ID: &'static str = "auth-fallback";
    const NAME: &'static str = "FallbackAuthenticator";

    fn into_handle(self) -> Self::Handle {
        Arc::new(self)
    }
}

fn auth_dependencies() -> Vec<DependencyDescriptor> {
    vec![<Cfg<AuthConfig> as FromContainer>::dependency()]
}

/// The boxed factory future the erased [`upwell_di::ComponentFactoryDescriptor`]
/// signature returns.
type AuthFactoryFuture<'a> =
    Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + 'a>>;

fn construct_primary(context: &mut ComponentConstructionContext) -> AuthFactoryFuture<'_> {
    Box::pin(async move {
        let cfg = <Cfg<AuthConfig> as FromContainer>::from_container(context).await?;
        let observed_mode = cfg.snapshot().mode.clone();

        OBSERVED_MODES.lock().await.push(observed_mode.clone());

        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<PrimaryAuthenticator>(PrimaryAuthenticator::NAME),
            value: Box::new(Injectable::into_stored(Arc::new(PrimaryAuthenticator {
                observed_mode,
            }))),
        })
    })
}

fn construct_fallback(context: &mut ComponentConstructionContext) -> AuthFactoryFuture<'_> {
    Box::pin(async move {
        let cfg = <Cfg<AuthConfig> as FromContainer>::from_container(context).await?;
        let observed_mode = cfg.snapshot().mode.clone();

        OBSERVED_MODES.lock().await.push(observed_mode.clone());

        Ok(BoxedComponent {
            ty: TypeDescriptor::of::<FallbackAuthenticator>(FallbackAuthenticator::NAME),
            value: Box::new(Injectable::into_stored(Arc::new(FallbackAuthenticator {
                observed_mode,
            }))),
        })
    })
}

/// Re-erases an already-built `Arc<T>` as `Arc<dyn Authenticator>` for storage under
/// the trait's key — the same job the `#[component(provide = ..)]` macro generates.
fn erase_authenticator<T>(boxed: &BoxedComponent) -> BoxedComponent
where
    T: Component<Handle = Arc<T>> + Authenticator,
{
    let live = boxed
        .value
        .downcast_ref::<Live<T>>()
        .expect("authenticator slot stores its own live handle");
    let erased: Arc<dyn Authenticator> = live.snapshot();

    BoxedComponent {
        ty: TypeDescriptor::of::<dyn Authenticator>("dyn Authenticator"),
        value: Box::new(Injectable::into_stored(erased)),
    }
}

fn erase_primary(boxed: &BoxedComponent) -> BoxedComponent {
    erase_authenticator::<PrimaryAuthenticator>(boxed)
}

fn erase_fallback(boxed: &BoxedComponent) -> BoxedComponent {
    erase_authenticator::<FallbackAuthenticator>(boxed)
}

fn primary_factories() -> &'static [ComponentFactoryDescriptor] {
    &[ComponentFactoryDescriptor {
        id: "static",
        construct: construct_primary,
        dependencies: auth_dependencies,
        default: true,
    }]
}

fn fallback_factories() -> &'static [ComponentFactoryDescriptor] {
    &[ComponentFactoryDescriptor {
        id: "static",
        construct: construct_fallback,
        dependencies: auth_dependencies,
        default: true,
    }]
}

static PRIMARY_PROVIDER: ProviderDescriptor = ProviderDescriptor {
    trait_ty: TypeDescriptor::of::<dyn Authenticator>("dyn Authenticator"),
    concrete_ty: TypeDescriptor::of::<PrimaryAuthenticator>(PrimaryAuthenticator::NAME),
    qualifier: "auth-primary",
    primary: true,
    priority: 0,
    ordering: &[],
    erase: erase_primary,
};

static FALLBACK_PROVIDER: ProviderDescriptor = ProviderDescriptor {
    trait_ty: TypeDescriptor::of::<dyn Authenticator>("dyn Authenticator"),
    concrete_ty: TypeDescriptor::of::<FallbackAuthenticator>(FallbackAuthenticator::NAME),
    qualifier: "auth-fallback",
    primary: true,
    priority: 0,
    ordering: &[],
    erase: erase_fallback,
};

static PRIMARY_COMPONENT: ComponentDescriptor = ComponentDescriptor {
    id: PrimaryAuthenticator::ID,
    name: PrimaryAuthenticator::NAME,
    ty: TypeDescriptor::of::<PrimaryAuthenticator>(PrimaryAuthenticator::NAME),
    scope: &Singleton,
    condition: Some(&PRIMARY_CONDITION),
    factories: primary_factories,
    hooks: upwell_hooks::no_hooks,
    generation_snapshot: None,
};

static FALLBACK_COMPONENT: ComponentDescriptor = ComponentDescriptor {
    id: FallbackAuthenticator::ID,
    name: FallbackAuthenticator::NAME,
    ty: TypeDescriptor::of::<FallbackAuthenticator>(FallbackAuthenticator::NAME),
    scope: &Singleton,
    condition: Some(&FALLBACK_CONDITION),
    factories: fallback_factories,
    hooks: upwell_hooks::no_hooks,
    generation_snapshot: None,
};

/// Builds a file-backed app with both provider components registered, the config
/// bound at `auth`, and the availability fact sourced from the same binding. The
/// temp dir is returned so the source file outlives the app and can be rewritten by
/// the test.
pub(super) async fn build_auth_app(mode: &str) -> (TempDir, App<()>) {
    let dir = tempfile::Builder::new()
        .prefix("upwell-auth-switch-")
        .tempdir()
        .expect("create temp dir");
    let config_dir = config_dir_of(&dir);

    write_auth_config(&config_dir, mode);

    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(&config_dir, &[], ResolverChain::empty())
            .expect("load config");

    let mut builder = App::<()>::builder("auth-provider-switch-test")
        .config_source(manager)
        .config::<AuthConfig>("auth")
        .condition_facts::<AuthConfig>("auth")
        .component_descriptor(&PRIMARY_COMPONENT)
        .component_descriptor(&FALLBACK_COMPONENT);

    builder
        .registry_mut()
        .providers
        .extend([PRIMARY_PROVIDER, FALLBACK_PROVIDER]);

    let app = builder.build().await.expect("auth app builds");

    (dir, app)
}

pub(super) fn config_dir_of(dir: &TempDir) -> PathBuf {
    let config_dir = dir.path().join("config");

    fs::create_dir_all(&config_dir).expect("create config dir");

    config_dir
}

pub(super) fn write_auth_config(config_dir: &Path, mode: &str) {
    fs::write(
        config_dir.join("application.toml"),
        format!("[auth]\nmode = \"{mode}\"\n"),
    )
    .expect("write auth config");
}
