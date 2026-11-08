use std::collections::BTreeSet;

use upwell_core::{Scope, ScopeId, Singleton};

use super::validate::{find_index, validate_sorted};
use super::{ScopeBoundary, ScopeParent, ScopeTopologyError};

/// A validated, immutable scope topology ready for planning and runtime lookup.
///
/// Boundaries are stored in stable identity order. The implicit root is available
/// to ancestry and reachability queries but is never returned by [`boundary`](Self::boundary).
/// Each boundary's common ancestors, and the ancestors on any of its chains, are computed once
/// here, so queries never walk the parent graph.
#[derive(Clone, Debug)]
pub struct PreparedScopeTopology {
    boundaries: Box<[ScopeBoundary]>,
    common_ancestors: Box<[Box<[ScopeId]>]>,
    any_ancestors: Box<[Box<[ScopeId]>]>,
}

impl PreparedScopeTopology {
    pub(super) fn new(boundaries: &[ScopeBoundary]) -> Result<Self, ScopeTopologyError> {
        let mut boundaries = boundaries.to_vec();

        boundaries.sort_by_key(|boundary| boundary.id());
        validate_sorted(&boundaries)?;

        let common_ancestors = common_ancestors(&boundaries);
        let any_ancestors = any_ancestors(&boundaries);

        Ok(Self {
            boundaries: boundaries.into_boxed_slice(),
            common_ancestors,
            any_ancestors,
        })
    }

    /// Returns all declared boundaries in stable identity order.
    pub fn boundaries(&self) -> &[ScopeBoundary] {
        &self.boundaries
    }

    /// Returns the declared boundary with `id`, if present.
    pub fn boundary(&self, id: &ScopeId) -> Option<&ScopeBoundary> {
        self.boundaries.get(find_index(&self.boundaries, id)?)
    }

    /// Returns whether `id` names a declared non-root boundary.
    pub fn contains(&self, id: &ScopeId) -> bool {
        self.boundary(id).is_some()
    }

    /// Returns the opening parents of `id`, if `id` is a boundary: the ones it declares, then
    /// the ones application scopes added to it.
    pub fn parents_of(&self, id: &ScopeId) -> Option<&'static [ScopeParent]> {
        self.boundary(id).map(ScopeBoundary::parents)
    }

    /// Returns the ancestors present on every chain of `id`, the implicit root included,
    /// in identity order. These are the scopes always open around `id`.
    pub fn common_ancestors(&self, id: &ScopeId) -> &[ScopeId] {
        find_index(&self.boundaries, id).map_or(&[], |index| &self.common_ancestors[index])
    }

    /// Returns whether a consumer boundary can reach a dependency boundary.
    ///
    /// A dependency is reachable when it is the consumer itself or an ancestor on every
    /// chain the consumer may be opened in. Siblings, and ancestors of only some chains,
    /// are unreachable regardless of lifetime rank.
    pub fn is_reachable(&self, consumer: &ScopeId, dependency: &ScopeId) -> bool {
        let root = Singleton::ID;

        if *consumer == root {
            return *dependency == root;
        }

        if !self.contains(consumer) {
            return false;
        }

        consumer == dependency
            || self
                .common_ancestors(consumer)
                .binary_search(dependency)
                .is_ok()
    }

    /// Returns whether `ancestor` encloses `descendant` on at least one opening chain.
    ///
    /// Unlike [`is_reachable`](Self::is_reachable), which asks for an ancestor on every
    /// chain, this holds when any chain opens `descendant` under `ancestor`. The root
    /// encloses every declared boundary and nothing encloses the root.
    pub fn encloses(&self, ancestor: &ScopeId, descendant: &ScopeId) -> bool {
        find_index(&self.boundaries, descendant)
            .is_some_and(|index| self.any_ancestors[index].binary_search(ancestor).is_ok())
    }
}

/// The ancestors on every opening chain of each boundary, sorted, indexed like `boundaries`.
///
/// A boundary's common ancestors are the intersection, over its parents, of the parent itself
/// plus the parent's own common ancestors. Requires validated parents and parent ranks: every
/// parent strictly outranks its child, so visiting boundaries from the highest rank down finds
/// each parent complete. The work is bounded by boundaries × parents × depth, never by the
/// number of distinct chains.
fn common_ancestors(boundaries: &[ScopeBoundary]) -> Box<[Box<[ScopeId]>]> {
    let mut order: Vec<usize> = (0..boundaries.len()).collect();
    let mut common: Vec<Box<[ScopeId]>> = vec![Box::new([]); boundaries.len()];

    order.sort_by_key(|index| std::cmp::Reverse(boundaries[*index].rank()));

    for index in order {
        common[index] = boundaries[index]
            .parents()
            .iter()
            .map(|parent| through_parent(boundaries, &common, *parent))
            .reduce(|left, right| {
                left.into_iter()
                    .filter(|id| right.binary_search(id).is_ok())
                    .collect()
            })
            .unwrap_or_default()
            .into_boxed_slice();
    }

    common.into_boxed_slice()
}

/// The ancestors on at least one opening chain of each boundary, sorted, indexed like
/// `boundaries`: the union, over its parents, of the parent itself plus the parent's own. Like
/// [`common_ancestors`], visits boundaries from the highest rank down, so each parent is
/// complete before its children read it.
fn any_ancestors(boundaries: &[ScopeBoundary]) -> Box<[Box<[ScopeId]>]> {
    let mut order: Vec<usize> = (0..boundaries.len()).collect();
    let mut any: Vec<Box<[ScopeId]>> = vec![Box::new([]); boundaries.len()];

    order.sort_by_key(|index| std::cmp::Reverse(boundaries[*index].rank()));

    for index in order {
        let ancestors: BTreeSet<ScopeId> = boundaries[index]
            .parents()
            .iter()
            .flat_map(|parent| through_parent(boundaries, &any, *parent))
            .collect();

        any[index] = ancestors.into_iter().collect();
    }

    any.into_boxed_slice()
}

/// The sorted ancestors of a boundary opened directly under `parent`.
fn through_parent(
    boundaries: &[ScopeBoundary],
    common: &[Box<[ScopeId]>],
    parent: ScopeParent,
) -> Vec<ScopeId> {
    let ScopeParent::Boundary(id) = parent else {
        return vec![Singleton::ID];
    };
    let index = find_index(boundaries, &id).expect("parent existence is validated before ancestry");
    let mut ancestors: Vec<ScopeId> = common[index].iter().copied().chain([id]).collect();

    ancestors.sort();

    ancestors
}
