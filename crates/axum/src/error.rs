//! The axum protocol's error type.

use thiserror::Error;

/// A failure raised while building or serving the axum protocol.
///
/// Absorbs the protocol-agnostic [`upwell_app::Error`] (so it satisfies the
/// `ProtocolDefinition::Error: From<app::Error>` bound) and the I/O errors raised by binding
/// and serving the TCP listener.
#[derive(Debug, Error)]
pub enum Error {
    /// A failure originating in the agnostic app/runtime layer (scope build, hooks, …).
    #[error(transparent)]
    App(#[from] upwell_app::Error),

    /// Binding the listener or serving the HTTP connection failed.
    #[error("axum serve error: {0}")]
    Serve(#[from] std::io::Error),

    /// A configuration value is internally inconsistent and would fail router construction — caught
    /// before mounting so it surfaces as an error rather than a build-time panic (e.g. conflicting
    /// OpenAPI `json_path`/`ui_path`).
    #[error("axum configuration error: {0}")]
    Config(String),

    /// Two different request seeders produce the same type, so every request scope would be
    /// filled twice.
    #[error("request seeders `{first}` and `{second}` both seed `{seed}` into the request scope")]
    DuplicateRequestSeeder {
        /// The seeded type.
        seed: &'static str,
        /// The seeder registered first.
        first: &'static str,
        /// The seeder registered second.
        second: &'static str,
    },

    /// A registered WebSocket protocol could not construct its endpoint state.
    #[error("building WebSocket protocol `{protocol}`: {source}")]
    WebsocketBuild {
        /// The concrete protocol type that failed.
        protocol: &'static str,

        /// The protocol-owned build failure.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// The axum protocol's [`Result`](std::result::Result) alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;
