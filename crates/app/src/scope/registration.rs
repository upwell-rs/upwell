//! Application-declared scopes: the `#[scope]` declarations an application adds to its
//! protocol's topology.

use upwell_core::{ItemSource, Scope, ScopeId, TypeDescriptor};
use upwell_di::ComponentDescriptor;

use super::{ScopeBoundary, ScopeParent};

/// One scope an application declares, such as a `#[scope]` type: its boundary in the topology,
/// the scopes that may open under it, and the slot its parameters fill.
///
/// Preparation adds the boundary to the active protocol's topology, adds the scope as a parent
/// of each of its `children`, and registers the parameter slot, so every instance can inject
/// its own value as `Arc<S>`.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct ScopeDeclaration {
    /// The scope and every parent it opens under.
    pub boundary: ScopeBoundary,
    /// Scopes declared elsewhere, such as a protocol's request scope, that may also open under
    /// this one. Each must already be a boundary of the topology and rank below this scope.
    pub children: &'static [ScopeId],
    /// The factory-less slot each instance's value fills, or `None` for a zero-sized scope,
    /// whose instances store nothing.
    pub params: Option<ComponentDescriptor>,
    /// The scope's parameter fields, in declaration order, for tooling.
    pub fields: &'static [&'static str],
    /// The scope's declaration, when the registering macro recorded it.
    pub source: Option<ItemSource>,
}

impl ScopeDeclaration {
    /// The declaration of scope `S`, opened under any of `parents`. A scope that is not
    /// zero-sized gets a parameter slot.
    pub const fn new<S: Scope>(parents: &'static [ScopeParent]) -> Self {
        Self {
            boundary: ScopeBoundary::new::<S>(parents),
            children: &[],
            params: params_slot::<S>(),
            fields: &[],
            source: None,
        }
    }

    /// Lets each of `children`, a scope declared elsewhere, also open under this one.
    pub const fn with_children(mut self, children: &'static [ScopeId]) -> Self {
        self.children = children;

        self
    }

    /// Records the scope's parameter fields.
    pub const fn with_fields(mut self, fields: &'static [&'static str]) -> Self {
        self.fields = fields;

        self
    }

    /// Records the scope's declaration.
    pub const fn declared_at(mut self, source: ItemSource) -> Self {
        self.source = Some(source);

        self
    }
}

/// Link-time registry of every `#[scope]` type's [`ScopeDeclaration`], folded into an app by
/// [`AppBuilder::auto_discover`](crate::AppBuilder::auto_discover).
#[linkme::distributed_slice]
pub static SCOPES: [ScopeDeclaration];

/// Whether an instance of scope `S` stores its value: every scope that is not zero-sized.
pub(crate) const fn stores_params<S: Scope>() -> bool {
    size_of::<S>() != 0
}

/// The rank of a scope opened directly under parents ranked `parents`: one below the
/// lowest-ranked of them, so it is shorter-lived than each.
///
/// Saturates at zero, which `#[scope]` rejects with a compile-time check: rank 0 belongs to the
/// transient scope.
#[doc(hidden)]
pub const fn rank_below(parents: &[u8]) -> u8 {
    let mut lowest = u8::MAX;
    let mut index = 0;

    while index < parents.len() {
        if parents[index] < lowest {
            lowest = parents[index];
        }

        index += 1;
    }

    lowest.saturating_sub(1)
}

/// The factory-less slot of scope `S`'s own value, unless `S` is zero-sized.
const fn params_slot<S: Scope>() -> Option<ComponentDescriptor> {
    if !stores_params::<S>() {
        return None;
    }

    Some(ComponentDescriptor::manual(
        S::ID.as_str(),
        S::NAME,
        TypeDescriptor::of::<S>(S::NAME),
        S::KEY,
    ))
}
