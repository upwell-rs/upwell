//! OpenAPI serving settings, bound beside the listener settings under the `openapi` feature.

use upwell_macros::config;

/// The property path the [`Axum`](crate::Axum) binds the OpenAPI settings at, under the
/// `openapi` feature. A subtree of `[axum]`, bound separately so its own field defaults apply.
pub const AXUM_OPENAPI_CONFIG_PATH: &str = "axum.openapi";

/// Which bundled OpenAPI UI the server mounts. Each non-`None` choice also requires its crate
/// feature (`openapi-swagger-ui`, `openapi-redoc`, `openapi-rapidoc`, `openapi-scalar`) to be
/// compiled; a choice whose feature is absent falls back to serving JSON only (with a warning).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenApiUi {
    /// Serve only `/openapi.json`, no UI.
    None,

    /// Swagger UI (`utoipa-swagger-ui`).
    Swagger,

    /// Redoc (`utoipa-redoc`).
    Redoc,

    /// RapiDoc (`utoipa-rapidoc`).
    Rapidoc,

    /// Scalar (`utoipa-scalar`).
    Scalar,
}

/// OpenAPI generation and serving settings, bound at [`AXUM_OPENAPI_CONFIG_PATH`].
///
/// Disabled by default: a build with the `openapi` feature still serves nothing until
/// `enabled = true`, so turning the spec on is an explicit deployment choice.
///
/// ```toml
/// [axum.openapi]
/// enabled = true
/// ui = "swagger"
/// ```
#[config(upwell = ::upwell_config)]
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
pub struct OpenApiConfig {
    /// Whether to assemble the document and mount the routes at all.
    #[default = "${AXUM_OPENAPI_ENABLED:false}"]
    pub enabled: bool,

    /// The path the JSON document is served at.
    #[default = "${AXUM_OPENAPI_JSON_PATH:/openapi.json}"]
    pub json_path: String,

    /// Which bundled UI to mount (or [`None`](OpenApiUi::None) for JSON only).
    #[default = "${AXUM_OPENAPI_UI:none}"]
    pub ui: OpenApiUi,

    /// The path the UI is mounted at (ignored when `ui = none`).
    #[default = "${AXUM_OPENAPI_UI_PATH:/docs}"]
    pub ui_path: String,

    /// The document title (OpenAPI `info.title`).
    #[default = "${AXUM_OPENAPI_TITLE:API}"]
    pub title: String,

    /// The document version (OpenAPI `info.version`).
    #[default = "${AXUM_OPENAPI_VERSION:0.0.0}"]
    pub version: String,
}

impl Default for OpenApiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            json_path: String::from("/openapi.json"),
            ui: OpenApiUi::None,
            ui_path: String::from("/docs"),
            title: String::from("API"),
            version: String::from("0.0.0"),
        }
    }
}
