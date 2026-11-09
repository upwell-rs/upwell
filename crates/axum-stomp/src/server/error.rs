//! STOMP-specific failures, distinct from the framing-agnostic [`WsDispatchError`].
//!
//! A [`StompError`] is what can go wrong *speaking* STOMP on a connection — a malformed frame, a
//! missing required header, no common protocol version — as opposed to dispatching a decoded
//! message to a handler (that stays [`WsDispatchError`]). It renders to an `ERROR` frame via the
//! server module's `error_frame` helper before the socket closes.

use upwell_axum::WsDispatchError;

use super::StompAuthenticationError;

/// A STOMP endpoint construction failure.
#[derive(Debug, thiserror::Error)]
pub enum StompBuildError {
    /// The protocol registration hook did not install its shared topic bus.
    #[error("STOMP topic bus is missing from the DI root")]
    MissingTopicBus,

    /// An authenticator argument only a scope provides. An authenticator runs before the
    /// connection scope opens, so it resolves its arguments from the application root.
    #[error(
        "STOMP authenticator argument `{dependency}` is provided only in scope `{scope}`; an authenticator runs before the connection scope opens and can inject singletons and configuration only"
    )]
    ScopedAuthenticatorDependency {
        /// The argument's type.
        dependency: &'static str,
        /// The scope that provides it.
        scope: &'static str,
    },
}

/// A STOMP protocol-level error on one connection.
#[derive(Debug, thiserror::Error)]
pub enum StompError {
    /// A received frame could not be parsed as STOMP (bad command, headers, or body framing).
    #[error("malformed STOMP frame: {0}")]
    Frame(String),

    /// A frame arrived whose command the server does not accept from a client in this state.
    #[error("unexpected STOMP command `{0}`")]
    UnexpectedCommand(String),

    /// The client offered no protocol version the server supports.
    #[error("no common STOMP version (client offered `{offered}`)")]
    VersionMismatch {
        /// The client's `accept-version` header, verbatim.
        offered: String,
    },

    /// The endpoint authenticator rejected the CONNECT credentials.
    #[error("authentication failed: {0}")]
    Authentication(#[from] StompAuthenticationError),

    /// The accepted connection's scope could not be opened, for example because an
    /// authenticator seed has no slot in it. The cause is logged.
    #[error("connection scope build failed")]
    ConnectionScope,

    /// A frame was missing a header the command requires.
    #[error("missing required header `{0}`")]
    MissingHeader(&'static str),

    /// A frame body exceeded the configured maximum.
    #[error("STOMP frame body too large ({size} > {max} bytes)")]
    TooLarge {
        /// The body size that was rejected.
        size: usize,
        /// The configured maximum.
        max: usize,
    },

    /// Dispatching a decoded message to its handler failed (decode/inject/handler error). Kept as a
    /// distinct arm so the framing-agnostic dispatch error flows through the STOMP error path too.
    #[error(transparent)]
    Dispatch(#[from] WsDispatchError),
}
