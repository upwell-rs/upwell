//! Component lifetime scopes.
//!
//! A scope describes where a component instance is stored and how long it lives. Stable
//! identity is distinct from its human-readable display label and lifetime rank. Scopes
//! are types implementing [`Scope`] (rather than a fixed enum) so a protocol can declare
//! its own lifetimes; descriptors carry a scope's [`ScopeKey`].
//!
//! The core knows only the two *universal anchors*: [`Singleton`] (one instance for the
//! whole application — the longest-lived scope, so [`u8::MAX`] rank) and [`Transient`]
//! (rebuilt on every resolution — the shortest-lived, so [`u8::MIN`] rank). Every other
//! scope a protocol defines necessarily ranks *between* them, so the captive-dependency
//! bounds hold by construction. Connection/request and any other protocol-shaped
//! lifetimes live in their protocol's crate, not here.

pub use crate::id::{IdErrorKind as InvalidScopeIdReason, InvalidNamespacedId as InvalidScopeId};

crate::namespaced_id_type!(
    /// A stable namespaced scope identity.
    pub struct ScopeId,
    "scope"
);

/// A component lifetime scope, identified by its type.
///
/// A scope type declares its stable ID, display label and lifetime rank as associated
/// constants. Descriptors and topologies carry the type's [`KEY`](Self::KEY), never a
/// reference to a scope value, so a scope type may later carry per-instance fields.
/// The captive-dependency rule (a longer-lived component may not depend on a
/// shorter-lived one) is enforced against [`RANK`](Self::RANK).
pub trait Scope: Send + Sync + 'static {
    /// Stable identity used to distinguish this scope from every other scope.
    const ID: ScopeId;

    /// Lifetime rank: longer-lived scopes rank higher. A non-transient component may
    /// depend only on equal-or-higher-ranked non-transient components.
    ///
    /// [`Singleton`] occupies [`u8::MAX`] and [`Transient`] [`u8::MIN`], so every
    /// protocol- or user-defined scope ranks strictly between them.
    const RANK: u8;

    /// Human-readable display label used in diagnostics.
    const NAME: &'static str;

    /// Whether this scope rebuilds its instance on every resolution rather than
    /// caching one per scope.
    const IS_TRANSIENT: bool = false;

    /// How an open instance of this scope relates to the runtime generations committed
    /// while it is open. Ignored for [`Singleton`] and [`Transient`], which the root and
    /// on-demand construction serve.
    const GENERATIONS: ScopeGenerations = ScopeGenerations::Pinned;

    /// The scope's identity and lifetime metadata as a copyable value.
    const KEY: ScopeKey = ScopeKey::of::<Self>();
}

/// A scope's identity and lifetime metadata, captured from its [`Scope`] type.
///
/// Keys compare and hash by [`id`](Self::id) alone: two keys with the same ID name the
/// same scope.
#[derive(Clone, Copy)]
pub struct ScopeKey {
    id: ScopeId,
    name: &'static str,
    rank: u8,
    transient: bool,
    generations: ScopeGenerations,
}

/// How an open scope relates to the runtime generations committed while it is open.
///
/// The policy is static, on the scope type: plans, layouts and the container kind are chosen
/// when a generation is prepared, never per open.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ScopeGenerations {
    /// Keeps the generation it opened under for its whole life: a call, request or message.
    /// A pinned scope opened under a durable one keeps its parent's generation.
    #[default]
    Pinned,
    /// Spans commits on the generation it opened under, such as a connection, and can
    /// report how many generations it has fallen behind.
    Durable,
}

impl ScopeGenerations {
    /// The policy's stable lowercase name, as tooling reports it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pinned => "pinned",
            Self::Durable => "durable",
        }
    }
}

impl ScopeKey {
    /// The key of scope type `S`.
    pub const fn of<S: Scope + ?Sized>() -> Self {
        Self {
            id: S::ID,
            name: S::NAME,
            rank: S::RANK,
            transient: S::IS_TRANSIENT,
            generations: S::GENERATIONS,
        }
    }

    /// Stable identity used for scope comparisons and validation.
    pub const fn id(&self) -> ScopeId {
        self.id
    }

    /// Lifetime rank: longer-lived scopes rank higher.
    pub const fn rank(&self) -> u8 {
        self.rank
    }

    /// Human-readable display label used in diagnostics.
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// Whether this scope rebuilds its instance on every resolution.
    pub const fn is_transient(&self) -> bool {
        self.transient
    }

    /// How an open instance of this scope relates to later runtime generations.
    pub const fn generations(&self) -> ScopeGenerations {
        self.generations
    }
}

impl PartialEq for ScopeKey {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for ScopeKey {}

impl std::hash::Hash for ScopeKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl std::fmt::Debug for ScopeKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScopeKey")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("rank", &self.rank)
            .field("transient", &self.transient)
            .field("generations", &self.generations)
            .finish()
    }
}

/// The root scope: a single instance for the whole application lifetime. The
/// longest-lived scope there can be, so it ranks [`u8::MAX`] — a singleton may depend
/// only on other singletons.
pub struct Singleton;

/// A transient scope: rebuilt fresh on every resolution and never cached. The
/// shortest-lived scope there can be, so it ranks [`u8::MIN`].
pub struct Transient;

impl Scope for Singleton {
    const ID: ScopeId = crate::namespaced_id!(ScopeId, "upwell/singleton");
    const RANK: u8 = u8::MAX;
    const NAME: &'static str = "Singleton";
}

impl Scope for Transient {
    const ID: ScopeId = crate::namespaced_id!(ScopeId, "upwell/transient");
    const RANK: u8 = u8::MIN;
    const NAME: &'static str = "Transient";
    const IS_TRANSIENT: bool = true;
}

#[cfg(test)]
mod tests;
