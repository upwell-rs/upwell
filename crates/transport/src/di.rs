//! DI-trait impls for [`PeerInfo`], behind the `di` feature.
//!
//! `PeerInfo` is a transport type the daemon seeds as a connection-scoped injectable. The
//! impls live here (PeerInfo's home crate) because the orphan rule forbids the daemon —
//! which owns neither `PeerInfo` nor the DI traits — from writing them.

use crate::PeerInfo;

/// The remote peer is a by-value connection-scoped injectable: a connection/request
/// component can depend on it directly as `peer: PeerInfo` — no `Arc`. Cheap to clone.
impl upwell_di::Injectable for PeerInfo {
    type Target = PeerInfo;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

/// Under `di-check`, `PeerInfo` is framework-seeded into every connection scope, so the
/// compile-time checker treats it as always provided.
#[cfg(feature = "di-check")]
impl upwell_di::Provided<PeerInfo> for upwell_di::Wiring {}
