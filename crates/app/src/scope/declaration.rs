use std::fmt;

use upwell_core::{Scope, ScopeId, ScopeKey, Singleton};

use super::{PreparedScopeTopology, ScopeTopologyError};

/// A parent a protocol-owned scope boundary may be opened under.
///
/// The application root is implicit and therefore is not itself a boundary in a
/// [`ScopeTopology`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ScopeParent {
    /// The framework-owned application root.
    Root,
    /// Another boundary declared by the same topology.
    Boundary(ScopeId),
}

impl ScopeParent {
    /// Creates a parent reference to a scope type.
    pub const fn of<T: Scope>() -> Self {
        Self::Boundary(T::ID)
    }

    /// Creates a parent reference to another declared boundary.
    pub const fn boundary(id: ScopeId) -> Self {
        Self::Boundary(id)
    }

    /// Returns the stable identity of this parent.
    pub const fn id(&self) -> ScopeId {
        match self {
            Self::Root => Singleton::ID,
            Self::Boundary(id) => *id,
        }
    }

    /// Returns whether this parent is the implicit application root.
    pub const fn is_root(&self) -> bool {
        matches!(self, Self::Root)
    }
}

/// One openable scope and every parent it may be opened under.
#[derive(Clone, Copy)]
pub struct ScopeBoundary {
    scope: ScopeKey,
    parents: &'static [ScopeParent],
}

impl ScopeBoundary {
    /// Declares an openable boundary for scope type `T`, opened under any of `parents`.
    pub const fn new<T: Scope>(parents: &'static [ScopeParent]) -> Self {
        Self {
            scope: T::KEY,
            parents,
        }
    }

    /// Returns the stable identity of this boundary.
    pub const fn id(&self) -> ScopeId {
        self.scope.id()
    }

    /// Returns the scope's identity and lifetime metadata.
    pub const fn scope(&self) -> ScopeKey {
        self.scope
    }

    /// Returns the diagnostic label captured from the scope type.
    pub const fn name(&self) -> &'static str {
        self.scope.name()
    }

    /// Returns the lifetime rank captured from the scope type.
    pub const fn rank(&self) -> u8 {
        self.scope.rank()
    }

    /// Returns how an open instance relates to later runtime generations, captured from the
    /// scope type.
    pub const fn generations(&self) -> upwell_core::ScopeGenerations {
        self.scope.generations()
    }

    /// Returns every parent this boundary may be opened under, in declaration order.
    pub const fn parents(&self) -> &'static [ScopeParent] {
        self.parents
    }

    /// Returns whether this boundary declares `parent` as one of its opening parents.
    pub fn opens_under(&self, parent: ScopeParent) -> bool {
        self.parents.contains(&parent)
    }
}

impl fmt::Debug for ScopeBoundary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScopeBoundary")
            .field("id", &self.id())
            .field("name", &self.name())
            .field("rank", &self.rank())
            .field("generations", &self.generations())
            .field("parents", &self.parents)
            .finish()
    }
}

/// An immutable declaration of protocol-owned scope boundaries.
///
/// The root is implicit. Every item in [`boundaries`](Self::boundaries) declares
/// exactly one non-root boundary and the parents it may be opened under. Call
/// [`prepare`](Self::prepare) before using a topology for planning or runtime opening.
#[derive(Clone, Copy, Debug)]
pub struct ScopeTopology {
    boundaries: &'static [ScopeBoundary],
}

impl ScopeTopology {
    /// Creates an empty topology for protocols without scoped boundaries.
    pub const fn empty() -> Self {
        Self::new(&[])
    }

    /// Creates a topology declaration from static scope boundaries.
    pub const fn new(boundaries: &'static [ScopeBoundary]) -> Self {
        Self { boundaries }
    }

    /// Returns the boundaries in declaration order.
    pub const fn boundaries(&self) -> &'static [ScopeBoundary] {
        self.boundaries
    }

    /// Validates and owns this topology for deterministic planning and lookup.
    pub fn prepare(&self) -> Result<PreparedScopeTopology, ScopeTopologyError> {
        PreparedScopeTopology::new(self.boundaries)
    }
}

impl Default for ScopeTopology {
    fn default() -> Self {
        Self::empty()
    }
}
