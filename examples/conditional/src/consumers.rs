//! The three `Authenticator` consumers, one per transition strategy a provider switch uses.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use upwell::{Cfg, Dep, Previous, component, methods};

use crate::auth::{AuthConfig, Authenticator};

/// Logs users in through whichever provider is current.
///
/// It holds a live `Dep<dyn Authenticator>`, so a provider switch rebinds the handle in place:
/// the gate instance survives every reload.
#[component]
pub struct LoginGate {
    authenticator: Dep<dyn Authenticator>,
}

impl LoginGate {
    /// The scheme that authenticated `token`, or `None` when it is rejected.
    pub fn login(&self, token: &str) -> Option<&'static str> {
        let authenticator = self.authenticator.get();

        authenticator.accepts(token).then(|| authenticator.scheme())
    }
}

/// Shows which scheme is active.
///
/// It captured a fixed `Arc<dyn Authenticator>` at construction, so a provider switch
/// reconstructs it with the ordinary factory.
#[component]
pub struct AuthBanner {
    authenticator: Arc<dyn Authenticator>,
}

impl AuthBanner {
    /// The banner text.
    pub fn text(&self) -> String {
        format!("signed in with the {} scheme", self.authenticator.scheme())
    }
}

/// Counts open sessions under a configured limit.
///
/// A provider switch replaces it through its state-aware `#[replace]` recipe, which carries
/// the open sessions into the new instance. The replacement fails when they exceed the new
/// limit, and the whole transition rolls back.
#[component]
pub struct SessionLedger {
    authenticator: Arc<dyn Authenticator>,
    #[default]
    limit: usize,
    #[default]
    sessions: AtomicUsize,
}

/// A replacement could not carry the open sessions under the new limit.
#[derive(Debug, thiserror::Error)]
#[error("{sessions} open sessions exceed the new limit of {limit}")]
pub struct SessionLimitExceeded {
    pub sessions: usize,
    pub limit: usize,
}

impl SessionLedger {
    /// Opens a session, returning how many are open.
    pub fn open(&self) -> usize {
        self.sessions.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// How many sessions are open.
    pub fn sessions(&self) -> usize {
        self.sessions.load(Ordering::SeqCst)
    }

    /// The most sessions this instance accepts.
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// The scheme the ledger was built for.
    pub fn scheme(&self) -> &'static str {
        self.authenticator.scheme()
    }
}

#[methods]
impl SessionLedger {
    #[init]
    fn new(authenticator: Arc<dyn Authenticator>, auth: Cfg<AuthConfig>) -> Self {
        Self {
            authenticator,
            limit: auth.snapshot().max_sessions,
            sessions: AtomicUsize::new(0),
        }
    }

    #[replace]
    fn carry(
        previous: Previous<Self>,
        authenticator: Arc<dyn Authenticator>,
        auth: Cfg<AuthConfig>,
    ) -> Result<Self, SessionLimitExceeded> {
        let sessions = previous.sessions();
        let limit = auth.snapshot().max_sessions;

        if sessions > limit {
            return Err(SessionLimitExceeded { sessions, limit });
        }

        Ok(Self {
            authenticator,
            limit,
            sessions: AtomicUsize::new(sessions),
        })
    }
}
