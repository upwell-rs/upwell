use std::fmt;
use std::sync::Mutex;

use upwell_core::{Scope, ScopeId, ScopeKey, Singleton};

use super::validate::{TopologyNode, validate_sorted};
use super::{PreparedScopeTopology, ScopeDeclaration, ScopeTopologyError};

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
    /// Creates a parent reference to a scope type: [`Root`](Self::Root) for [`Singleton`],
    /// which stands for the application root, and a boundary otherwise.
    pub const fn of<T: Scope>() -> Self {
        if same_id(T::ID, Singleton::ID) {
            return Self::Root;
        }

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

/// Whether two scope identities are equal, in a const context.
const fn same_id(left: ScopeId, right: ScopeId) -> bool {
    let left = left.as_str().as_bytes();
    let right = right.as_str().as_bytes();
    let mut index = 0;

    if left.len() != right.len() {
        return false;
    }

    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }

        index += 1;
    }

    true
}

/// Every parent list a successful preparation extended, kept for the life of the process so a
/// boundary stays `Copy` and its parents `'static`. A list is interned only once its whole
/// topology validated, and only once per distinct list, so the set is bounded by the distinct
/// valid extended lists the program's scope declarations can form.
static EXTENDED_PARENTS: Mutex<Vec<&'static [ScopeParent]>> = Mutex::new(Vec::new());

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

    /// Returns every parent this boundary may be opened under, in declaration order, followed
    /// by the parents application scopes added to it as their `children`, in registration
    /// order.
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

    /// Validates and owns this topology extended with the application's own `declared`
    /// scopes, such as the ones `#[scope]` registers. Each declared scope's boundary is added,
    /// and the scope is added as a parent of every boundary it names as a child, after the
    /// parents that boundary declares.
    ///
    /// # Errors
    ///
    /// [`MissingChild`](ScopeTopologyError::MissingChild) when a declared scope names a child
    /// no boundary of the extended topology declares, and every error
    /// [`prepare`](Self::prepare) reports, such as a child listing the scope as a parent
    /// already ([`DuplicateParent`](ScopeTopologyError::DuplicateParent)) or one that does
    /// not rank below it ([`InvalidParentRank`](ScopeTopologyError::InvalidParentRank)).
    pub fn prepare_with<'d>(
        &self,
        declared: impl IntoIterator<Item = &'d ScopeDeclaration>,
    ) -> Result<PreparedScopeTopology, ScopeTopologyError> {
        let declared: Vec<&ScopeDeclaration> = declared.into_iter().collect();
        let mut drafts: Vec<DraftBoundary> = self
            .boundaries
            .iter()
            .copied()
            .chain(declared.iter().map(|declaration| declaration.boundary))
            .map(DraftBoundary::new)
            .collect();

        for declaration in &declared {
            add_to_children(&mut drafts, declaration)?;
        }

        drafts.sort_by_key(TopologyNode::id);
        validate_sorted(&drafts)?;

        let boundaries: Vec<ScopeBoundary> =
            drafts.into_iter().map(DraftBoundary::finish).collect();

        PreparedScopeTopology::new(&boundaries)
    }
}

impl Default for ScopeTopology {
    fn default() -> Self {
        Self::empty()
    }
}

/// A boundary whose parents application scopes may still extend, validated before its parent
/// list is interned.
struct DraftBoundary {
    boundary: ScopeBoundary,
    parents: Vec<ScopeParent>,
}

impl DraftBoundary {
    fn new(boundary: ScopeBoundary) -> Self {
        Self {
            boundary,
            parents: boundary.parents().to_vec(),
        }
    }

    fn add_parent(&mut self, parent: ScopeParent) {
        self.parents.push(parent);
    }

    /// The validated boundary, with its extended parents interned when any were added.
    fn finish(self) -> ScopeBoundary {
        if self.parents.len() == self.boundary.parents().len() {
            return self.boundary;
        }

        ScopeBoundary {
            scope: self.boundary.scope,
            parents: intern_parents(self.parents),
        }
    }
}

impl TopologyNode for DraftBoundary {
    fn id(&self) -> ScopeId {
        self.boundary.id()
    }

    fn rank(&self) -> u8 {
        self.boundary.rank()
    }

    fn parents(&self) -> &[ScopeParent] {
        &self.parents
    }
}

/// Adds the scope `declaration` declares as a parent of every boundary it names as a child.
fn add_to_children(
    drafts: &mut [DraftBoundary],
    declaration: &ScopeDeclaration,
) -> Result<(), ScopeTopologyError> {
    let id = declaration.boundary.id();
    let parent = ScopeParent::Boundary(id);

    for child in declaration.children {
        let draft = drafts
            .iter_mut()
            .find(|draft| TopologyNode::id(*draft) == *child)
            .ok_or(ScopeTopologyError::MissingChild { id, child: *child })?;

        draft.add_parent(parent);
    }

    Ok(())
}

/// How many extended parent lists the process has interned.
#[cfg(test)]
pub(super) fn interned_parent_lists() -> usize {
    EXTENDED_PARENTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .len()
}

/// The interned, `'static` copy of `parents`.
///
/// A poisoned lock is used as is: the list only ever grows by one complete entry under it, so a
/// panic elsewhere cannot leave it half-written.
fn intern_parents(parents: Vec<ScopeParent>) -> &'static [ScopeParent] {
    let mut interned = EXTENDED_PARENTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    if let Some(existing) = interned
        .iter()
        .find(|existing| **existing == parents.as_slice())
    {
        return existing;
    }

    let leaked: &'static [ScopeParent] = Box::leak(parents.into_boxed_slice());

    interned.push(leaked);

    leaked
}
