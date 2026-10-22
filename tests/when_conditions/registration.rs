//! The conditional custom provider behaves the same however it is registered: manually,
//! through link-time auto-discovery, or as a plugin contribution, each switch commits in
//! both directions and moves the fixed gate with it.

use std::fs;
use std::path::Path;

use upwell::config::Toml;
use upwell::{
    App, AppBuilder, ComponentDescriptor, ConfigManager, Descriptor, Plugin, PluginContributions,
    PluginId, ProviderDescriptor,
};
use upwell_config::ResolverChain;

use crate::app::{AuthGate, CustomAuthenticator, DefaultAuthenticator, Settings, write_settings};

const DEFAULT: Settings = Settings {
    mode: "default",
    audit: false,
    export: None,
};

const CUSTOM: Settings = Settings {
    mode: "custom",
    audit: false,
    export: None,
};

/// Contributes the conditional custom provider and its trait mapping.
#[derive(Default)]
struct CustomAuthPlugin;

impl Plugin for CustomAuthPlugin {
    const ID: PluginId = upwell::namespaced_id!(PluginId, "test/custom-auth");

    fn contribute(self, contributions: &mut PluginContributions) {
        let providers =
            <CustomAuthenticator as Descriptor<&'static [ProviderDescriptor]>>::DESCRIPTOR;

        contributions.component_descriptor(
            upwell::namespaced_id!(upwell::ContributionId, "test/custom-auth"),
            <CustomAuthenticator as Descriptor<ComponentDescriptor>>::DESCRIPTOR,
        );

        for provider in providers {
            contributions.provider(
                upwell::namespaced_id!(upwell::ContributionId, "test/custom-auth-provider"),
                *provider,
            );
        }
    }
}

/// How the custom provider reaches the application.
#[derive(Clone, Copy, Debug)]
enum Registration {
    Manual,
    Discovered,
    Plugin,
}

fn builder(config_dir: &Path, registration: Registration) -> AppBuilder<()> {
    let manager =
        ConfigManager::<Toml>::load_in_with_resolvers(config_dir, &[], ResolverChain::empty())
            .expect("load config");
    let builder = App::<()>::builder("when-conditions-registration").config_source(manager);

    match registration {
        Registration::Manual => builder
            .component::<DefaultAuthenticator>()
            .component::<CustomAuthenticator>()
            .component::<AuthGate>()
            .provider::<DefaultAuthenticator>()
            .provider::<CustomAuthenticator>(),
        Registration::Discovered => builder.auto_discover(),
        Registration::Plugin => builder
            .component::<DefaultAuthenticator>()
            .component::<AuthGate>()
            .provider::<DefaultAuthenticator>()
            .register_plugin::<CustomAuthPlugin>(),
    }
}

#[tokio::test]
async fn every_registration_source_switches_the_provider_in_both_directions() {
    for registration in [
        Registration::Manual,
        Registration::Discovered,
        Registration::Plugin,
    ] {
        let dir = tempfile::Builder::new()
            .prefix("upwell-when-registration-")
            .tempdir()
            .expect("create temp dir");
        let config_dir = dir.path().join("config");

        fs::create_dir_all(&config_dir).expect("create config dir");
        write_settings(&config_dir, &DEFAULT);

        let app = builder(&config_dir, registration)
            .build()
            .await
            .unwrap_or_else(|error| panic!("{registration:?}: the app builds: {error}"));
        let runtime = app.runtime();
        let scheme = || runtime.root().get::<AuthGate>().expect("gate").scheme();

        assert_eq!(
            scheme(),
            "default",
            "{registration:?}: starts on the default"
        );

        for (settings, expected) in [(&CUSTOM, "custom"), (&DEFAULT, "default")] {
            write_settings(&config_dir, settings);

            let report = runtime
                .reload_config()
                .await
                .unwrap_or_else(|error| panic!("{registration:?}: the switch commits: {error}"));

            assert!(report.is_committed(), "{registration:?}: committed");
            assert_eq!(scheme(), expected, "{registration:?}: the gate follows");
        }
    }
}
