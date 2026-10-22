//! Tooling preparation of the `Authenticator` app: pure `#[when]` conditions are evaluated
//! for the supplied config, a condition outside the subset is shown with its source as not
//! evaluated, and the projection names the provider the consumer selects.

use std::cell::RefCell;
use std::path::PathBuf;

use upwell::tooling::{Resource, ToolingDocument};
use upwell::{AppBuilder, AppHost, ExecutionMode, prepare_host};

use crate::app::{Settings, builder, write_settings};

thread_local! {
    /// The config directory the tooling host prepares over; each test owns its thread.
    static CONFIG_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// The `Authenticator` app as a host, prepared over the test's config directory.
struct AuthHost;

impl AppHost for AuthHost {
    type Protocol = ();

    fn builder() -> Result<AppBuilder<()>, upwell::ConfigError> {
        let config_dir = CONFIG_DIR.with_borrow(|dir| dir.clone().expect("the test set a dir"));

        Ok(builder(&config_dir))
    }
}

async fn inspect(mode: &'static str) -> ToolingDocument {
    let dir = tempfile::Builder::new()
        .prefix("upwell-when-tooling-")
        .tempdir()
        .expect("create temp dir");
    let config_dir = dir.path().join("config");

    std::fs::create_dir_all(&config_dir).expect("create config dir");
    write_settings(
        &config_dir,
        &Settings {
            mode,
            audit: false,
            export: None,
        },
    );
    CONFIG_DIR.with_borrow_mut(|dir| *dir = Some(config_dir));

    let (_, prepared) = prepare_host::<AuthHost>(ExecutionMode::Tooling)
        .await
        .expect("the decidable graph prepares");

    prepared
        .tooling_document()
        .expect("prepared state projects")
}

fn resource<'a>(document: &'a ToolingDocument, id: &str) -> &'a Resource {
    document
        .resources
        .iter()
        .find(|resource| resource.id == id)
        .unwrap_or_else(|| panic!("resource `{id}` is projected"))
}

fn label<'a>(document: &'a ToolingDocument, id: &str, name: &str) -> &'a str {
    resource(document, id)
        .labels
        .get(name)
        .unwrap_or_else(|| panic!("resource `{id}` carries label `{name}`"))
}

/// The concrete Rust type of the provider the gate's `dyn Authenticator` resolves to.
fn selected_provider(document: &ToolingDocument) -> &str {
    document
        .relationships
        .iter()
        .find(|relationship| {
            relationship.from == "component:authgate"
                && relationship.labels.get("role").map(String::as_str) == Some("resolved-provider")
        })
        .map(|relationship| relationship.to.as_str())
        .expect("the gate resolves one provider")
}

/// The single condition node of a one-`#[when]` component.
fn condition_node<'a>(document: &'a ToolingDocument, component: &str) -> &'a Resource {
    document
        .resources
        .iter()
        .find(|resource| resource.id.starts_with(&format!("condition:{component}:")))
        .unwrap_or_else(|| panic!("`{component}` has a condition node"))
}

#[tokio::test(flavor = "current_thread")]
async fn tooling_evaluates_the_custom_provider_condition_for_the_supplied_config() {
    let custom = inspect("custom").await;
    let default = inspect("default").await;

    for (document, state, selected) in [
        (&custom, "holds", "CustomAuthenticator"),
        (&default, "fails", "DefaultAuthenticator"),
    ] {
        let node = condition_node(document, "customauthenticator");

        assert_eq!(
            label(document, "component:customauthenticator", "condition-state"),
            state
        );
        assert_eq!(node.labels["outcome"], state);
        assert_eq!(node.labels["purity"], "pure");
        assert_eq!(
            node.labels["expression"],
            "AuthConfig.mode == AuthMode::Custom"
        );
        assert!(
            selected_provider(document).contains(selected),
            "the gate selects {selected}"
        );
        assert_eq!(
            label(document, "component:defaultauthenticator", "effective"),
            "true"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_condition_outside_the_subset_is_shown_as_not_evaluated() {
    let document = inspect("custom").await;

    let node = condition_node(&document, "trustedcondition");

    assert_eq!(node.labels["outcome"], "not-evaluated");
    assert_eq!(node.labels["purity"], "trusted");
    assert_eq!(
        node.labels["expression"],
        "matches!(AuthConfig.mode, AuthMode::Custom)"
    );
    assert_eq!(
        label(&document, "component:trustedcondition", "effective"),
        "false"
    );
    assert_eq!(
        label(&document, "application", "effective-graph"),
        "partial"
    );
}
