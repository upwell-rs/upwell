//! OpenAPI document assembly for the axum protocol.
//!
//! The `#[dto]` and `#[handlers]` macros lower per-type and per-route metadata into two link-time
//! slices — [`OPENAPI_SCHEMAS`] (one entry per `#[dto]`, contributing its `utoipa::ToSchema`
//! component and its transitive dependencies) and [`OPENAPI_OPERATIONS`] (one entry per HTTP route,
//! contributing a `utoipa::openapi::path::Operation` built by `#[utoipa::path]`). [`build_openapi`]
//! folds both into a single [`utoipa::openapi::OpenApi`] at serve time; the model, schema
//! generation, and serialization are entirely utoipa's — this module only collects and joins.
//!
//! Mirrors the `CONTROLLERS` linkme pattern, so operations/schemas from independent `#[handlers]`
//! blocks and separate crates all compose with no central registration.

use utoipa::openapi::path::{HttpMethod, Operation};
use utoipa::openapi::schema::Schema;
use utoipa::openapi::{ComponentsBuilder, Info, OpenApi, OpenApiBuilder, Paths, RefOr, Server};

use crate::config::{OpenApiConfig, OpenApiUi};

/// One HTTP route's contribution to the document: its full path, the HTTP methods it serves, and
/// the utoipa [`Operation`]. Produced by a `#[handlers]`-generated closure over the route's
/// `#[utoipa::path]` type; the path is joined onto the controller base at call time.
pub type OperationEntry = fn() -> (String, Vec<HttpMethod>, Operation);

/// One `#[dto]` type's contribution to the document's components: it pushes its own
/// `(name, schema)` and those of its nested `#[dto]` fields, via `utoipa::ToSchema::schemas`.
pub type SchemaEntry = fn(&mut Vec<(String, RefOr<Schema>)>);

/// Every HTTP route registers one [`OperationEntry`] here (native + `openapi` only), mirroring the
/// `CONTROLLERS` slice. Folded by [`build_openapi`].
#[linkme::distributed_slice]
pub static OPENAPI_OPERATIONS: [OperationEntry];

/// Every `#[dto]` registers one [`SchemaEntry`] here, contributing its component schema.
#[linkme::distributed_slice]
pub static OPENAPI_SCHEMAS: [SchemaEntry];

/// Joins a controller `base` with a route's relative path the same way the router mounts it
/// (`Router::nest(base, ..)`): `("/users", "/{id}")` → `/users/{id}`; an empty or `"/"` base yields
/// the relative path; an empty relative path yields the base. The result always starts with `/`.
pub fn join_base(base: &str, relative: &str) -> String {
    let base = base.trim_end_matches('/');
    let relative = if relative == "/" { "" } else { relative };

    let joined = format!("{base}{relative}");

    if joined.is_empty() {
        String::from("/")
    } else {
        joined
    }
}

/// Folds the two link-time slices into a single OpenAPI document with the given `title`/`version`.
/// Operations sharing a path are merged into one path item (utoipa handles the per-method merge);
/// component schemas are deduped by name. A non-empty `base_path` becomes an OpenAPI `server` URL,
/// so documented operation paths stay relative to it (matching how the router nests under the
/// prefix). Pure — safe to call once at serve time.
pub fn build_openapi(title: &str, version: &str, base_path: &str) -> OpenApi {
    let mut paths = Paths::new();
    let mut schemas: Vec<(String, RefOr<Schema>)> = Vec::new();

    for entry in OPENAPI_OPERATIONS {
        let (path, methods, operation) = entry();

        paths.add_path_operation(&path, methods, operation);
    }

    for entry in OPENAPI_SCHEMAS {
        entry(&mut schemas);
    }

    let components = ComponentsBuilder::new().schemas_from_iter(schemas).build();
    let servers = normalize_prefix(base_path).map(|prefix| vec![Server::new(prefix)]);

    OpenApiBuilder::new()
        .info(Info::new(title, version))
        .paths(paths)
        .servers(servers)
        .components(Some(components))
        .build()
}

/// Whether the crate feature backing `ui` is compiled into this build. A selected UI whose feature
/// is absent is not mounted (JSON-only fallback), so callers that only care about *actually mounted*
/// routes gate on this rather than on the configured choice alone.
fn ui_is_compiled(ui: OpenApiUi) -> bool {
    match ui {
        OpenApiUi::None => false,
        OpenApiUi::Swagger => cfg!(feature = "openapi-swagger-ui"),
        OpenApiUi::Redoc => cfg!(feature = "openapi-redoc"),
        OpenApiUi::Rapidoc => cfg!(feature = "openapi-rapidoc"),
        OpenApiUi::Scalar => cfg!(feature = "openapi-scalar"),
    }
}

/// The absolute URL a UI must fetch the spec from: the normalized base prefix followed by the
/// configured `json_path`. Under `base_path = /api` the JSON route nests to `/api{json_path}`, so a
/// UI handed the bare `json_path` would request the wrong (root) location and fail to load the spec.
fn spec_url(base_path: &str, json_path: &str) -> String {
    format!(
        "{}{}",
        normalize_prefix(base_path).unwrap_or_default(),
        json_path
    )
}

/// Normalizes a configured base path into an OpenAPI server prefix: `None` for an empty or `"/"`
/// path (routes at the root), otherwise the path with any trailing slash trimmed.
fn normalize_prefix(base_path: &str) -> Option<String> {
    let trimmed = base_path.trim_end_matches('/');

    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Mounts the OpenAPI surface onto `router` when [enabled](OpenApiConfig::enabled): the JSON
/// document at [`json_path`](OpenApiConfig::json_path), plus the configured UI (or none). The
/// document is built once here. A UI whose crate feature is not compiled logs a warning and is
/// skipped, leaving the JSON endpoint intact. Returns `router` unchanged when disabled.
///
/// `base_path` is the (already-normalized) global prefix the router will be nested under; it is
/// recorded as the document's server URL, and prepended to the spec URL handed to a UI so the UI
/// fetches the document at its real, prefixed location. The routes mounted here are relative — they
/// ride the same nesting.
///
/// Returns [`Error::Config`](crate::Error::Config) when the configured `json_path` and `ui_path`
/// overlap, since mounting both would register conflicting routes and panic during router
/// construction.
pub fn mount(
    router: axum::Router,
    config: &OpenApiConfig,
    base_path: &str,
) -> crate::Result<axum::Router> {
    if !config.enabled {
        return Ok(router);
    }

    validate_config(config)?;

    let plan = MountPlan::of(config);
    let doc = build_openapi(&config.title, &config.version, base_path);
    let spec_url = spec_url(base_path, &config.json_path);

    let router = match plan.json {
        JsonServer::Framework => serve_json(router, &config.json_path, doc.clone()),
        JsonServer::SwaggerUi => router,
    };

    Ok(mount_ui(router, config, doc, &spec_url))
}

/// What the OpenAPI surface mounts for one enabled configuration — the single decision both
/// [`mount`] and the tooling projection read, so the routes tooling reports are the routes served.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MountPlan {
    /// Who serves the JSON document at `json_path`.
    pub(crate) json: JsonServer,
    /// The UI actually mounted at `ui_path`: configured *and* compiled into this build.
    pub(crate) ui: Option<OpenApiUi>,
}

/// The server of the OpenAPI JSON document route.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum JsonServer {
    /// The framework's own `GET {json_path}` route.
    Framework,
    /// Swagger UI, which serves its own copy of the document at `json_path`.
    SwaggerUi,
}

impl MountPlan {
    /// The plan for an enabled `config`. Swagger UI serves its own copy of the spec; every other
    /// choice (a UI that inlines the doc, references our endpoint, or no UI) needs ours.
    pub(crate) fn of(config: &OpenApiConfig) -> Self {
        let ui = ui_is_compiled(config.ui).then_some(config.ui);
        let json = if ui == Some(OpenApiUi::Swagger) {
            JsonServer::SwaggerUi
        } else {
            JsonServer::Framework
        };

        Self { json, ui }
    }
}

/// The routes [`mount`] serves for `config`, relative to the global base path, for tooling.
///
/// Read from the same [`MountPlan`] as [`mount`], so a disabled surface or a UI whose crate feature
/// is absent reports exactly what the router serves.
#[cfg(feature = "tooling")]
pub(crate) fn mounted_routes(config: &OpenApiConfig) -> Vec<crate::plugin::tooling::MountedRoute> {
    use crate::plugin::tooling::{MountedRoute, MountedRouteOwner};

    let mut routes = Vec::new();

    if !config.enabled {
        return routes;
    }

    let plan = MountPlan::of(config);
    let json_summary = match plan.json {
        JsonServer::Framework => String::from("OpenAPI document"),
        JsonServer::SwaggerUi => String::from("OpenAPI document (served by Swagger UI)"),
    };

    routes.push(MountedRoute {
        owner: MountedRouteOwner::OpenApi,
        method: "GET",
        path: config.json_path.clone(),
        handler: "openapi_json",
        summary: json_summary,
        source: upwell_core::item_source!("serve_json"),
    });

    if let Some(ui) = plan.ui {
        routes.push(MountedRoute {
            owner: MountedRouteOwner::OpenApi,
            method: "GET",
            path: config.ui_path.clone(),
            handler: "openapi_ui",
            summary: format!("{ui:?} OpenAPI UI"),
            source: upwell_core::item_source!("mount_ui"),
        });
    }

    routes
}

/// Rejects a configuration whose JSON and UI routes would collide: identical paths, or a `json_path`
/// nested under `ui_path` (a UI's wildcard would then also claim it). Only checked when the selected
/// UI is **actually compiled** — a UI whose crate feature is absent falls back to serving JSON only
/// (see [`mount_ui`]), which has nothing to collide with, so overlapping paths are then harmless.
pub(crate) fn validate_config(config: &OpenApiConfig) -> crate::Result<()> {
    if !config.enabled {
        return Ok(());
    }

    crate::config::validate_mount_path("OpenAPI JSON path", &config.json_path)?;

    if !ui_is_compiled(config.ui) {
        return Ok(());
    }

    crate::config::validate_mount_path("OpenAPI UI path", &config.ui_path)?;

    let json = config.json_path.trim_end_matches('/');
    let ui = config.ui_path.trim_end_matches('/');

    let overlaps = json == ui || (!ui.is_empty() && json.starts_with(&format!("{ui}/")));

    if overlaps {
        return Err(crate::Error::Config(format!(
            "OpenAPI json_path (`{}`) overlaps ui_path (`{}`): serving both would register \
             conflicting routes; configure distinct paths",
            config.json_path, config.ui_path
        )));
    }

    Ok(())
}

/// Adds a `GET {json_path}` route returning the serialized document.
fn serve_json(router: axum::Router, json_path: &str, doc: OpenApi) -> axum::Router {
    router.route(
        json_path,
        axum::routing::get(move || {
            let body = doc.clone();

            async move { axum::Json(body) }
        }),
    )
}

/// Mounts the configured UI onto `router`, if any. Each arm is gated on its crate feature; a UI
/// selected in config whose feature is absent logs a warning and serves JSON only. `spec_url` is the
/// prefixed URL a UI that fetches the spec by URL (Swagger, RapiDoc) must request it from — the
/// UIs that inline the document (Redoc, Scalar) ignore it.
fn mount_ui(
    router: axum::Router,
    config: &OpenApiConfig,
    doc: OpenApi,
    spec_url: &str,
) -> axum::Router {
    match config.ui {
        OpenApiUi::None => {
            let _ = spec_url;

            router
        }

        OpenApiUi::Swagger => {
            #[cfg(feature = "openapi-swagger-ui")]
            {
                // `.url` sets the route the spec is *served* at (relative, so nesting under
                // `base_path` places it correctly); `.config` sets the URL the UI *fetches* — the
                // base-prefixed `spec_url`. Passing the prefix to `.url` instead would nest twice
                // (`/api/api/openapi.json`), so the two are configured separately.
                let swagger = utoipa_swagger_ui::SwaggerUi::new(config.ui_path.clone())
                    .url(config.json_path.clone(), doc)
                    .config(utoipa_swagger_ui::Config::new([spec_url.to_owned()]));

                router.merge(swagger)
            }

            #[cfg(not(feature = "openapi-swagger-ui"))]
            {
                warn_missing_ui("swagger", "openapi-swagger-ui");

                let _ = (doc, spec_url);

                router
            }
        }

        OpenApiUi::Redoc => {
            #[cfg(feature = "openapi-redoc")]
            {
                use utoipa_redoc::Servable;

                router.merge(utoipa_redoc::Redoc::with_url(config.ui_path.clone(), doc))
            }

            #[cfg(not(feature = "openapi-redoc"))]
            {
                warn_missing_ui("redoc", "openapi-redoc");

                let _ = doc;

                router
            }
        }

        OpenApiUi::Rapidoc => {
            #[cfg(feature = "openapi-rapidoc")]
            {
                // RapiDoc references the spec by URL, so it must be the prefixed `spec_url` (the real,
                // nested location of our JSON route); the inlined doc is unused.
                let _ = doc;
                let rapidoc =
                    utoipa_rapidoc::RapiDoc::new(spec_url.to_owned()).path(config.ui_path.clone());

                router.merge(rapidoc)
            }

            #[cfg(not(feature = "openapi-rapidoc"))]
            {
                warn_missing_ui("rapidoc", "openapi-rapidoc");

                let _ = (doc, spec_url);

                router
            }
        }

        OpenApiUi::Scalar => {
            #[cfg(feature = "openapi-scalar")]
            {
                use utoipa_scalar::Servable;

                router.merge(utoipa_scalar::Scalar::with_url(config.ui_path.clone(), doc))
            }

            #[cfg(not(feature = "openapi-scalar"))]
            {
                warn_missing_ui("scalar", "openapi-scalar");

                let _ = doc;

                router
            }
        }
    }
}

/// Warns that a configured UI cannot be served because its crate feature is not compiled.
#[allow(dead_code)]
fn warn_missing_ui(ui: &str, feature: &str) {
    tracing::warn!(
        target: "upwell::axum",
        ui,
        feature,
        "OpenAPI UI selected in config but its crate feature is not enabled; serving JSON only"
    );
}

#[cfg(test)]
mod tests;
