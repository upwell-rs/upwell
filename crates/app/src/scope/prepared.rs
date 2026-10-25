use upwell_core::{Scope, ScopeId, Singleton};

use super::validate::{
    find_index, validate_cycles, validate_ids, validate_parent_ranks, validate_parents,
};
use super::{ScopeBoundary, ScopeParent, ScopeTopologyError};

/// Every opening chain of one boundary, each from its immediate parent through the root.
type Chains = Box<[Box<[ScopeId]>]>;

/// A validated, immutable scope topology ready for planning and runtime lookup.
///
/// Boundaries are stored in stable identity order. The implicit root is available
/// to chain and reachability queries but is never returned by [`boundary`](Self::boundary).
/// Every opening chain is computed once here, so queries never walk the parent graph.
#[derive(Clone, Debug)]
pub struct PreparedScopeTopology {
    boundaries: Box<[ScopeBoundary]>,
    chains: Box<[Chains]>,
    common_ancestors: Box<[Box<[ScopeId]>]>,
}

impl PreparedScopeTopology {
    pub(super) fn new(boundaries: &[ScopeBoundary]) -> Result<Self, ScopeTopologyError> {
        let mut boundaries = boundaries.to_vec();

        boundaries.sort_by_key(|boundary| boundary.id());
        validate_ids(&boundaries)?;
        validate_parents(&boundaries)?;
        validate_cycles(&boundaries)?;
        validate_parent_ranks(&boundaries)?;

        let chains = opening_chains(&boundaries);
        let common_ancestors = chains.iter().map(|chains| common_ids(chains)).collect();

        Ok(Self {
            boundaries: boundaries.into_boxed_slice(),
            chains,
            common_ancestors,
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

    /// Returns the declared opening parents of `id`, if `id` is a boundary.
    pub fn parents_of(&self, id: &ScopeId) -> Option<&'static [ScopeParent]> {
        self.boundary(id).map(ScopeBoundary::parents)
    }

    /// Returns every chain `id` may be opened in.
    ///
    /// Each chain lists the boundary's ancestors from its immediate parent through the
    /// implicit root. An undeclared ID and the root itself have no chains.
    pub fn chains(&self, id: &ScopeId) -> &[Box<[ScopeId]>] {
        find_index(&self.boundaries, id).map_or(&[], |index| &self.chains[index])
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
}

/// Every opening chain of each boundary, indexed like `boundaries`.
///
/// Requires validated parents and parent ranks: every parent strictly outranks its child,
/// so visiting boundaries from the highest rank down finds each parent's chains complete.
fn opening_chains(boundaries: &[ScopeBoundary]) -> Box<[Chains]> {
    let mut order: Vec<usize> = (0..boundaries.len()).collect();
    let mut chains: Vec<Chains> = vec![Box::new([]); boundaries.len()];

    order.sort_by_key(|index| std::cmp::Reverse(boundaries[*index].rank()));

    for index in order {
        chains[index] = boundaries[index]
            .parents()
            .iter()
            .flat_map(|parent| parent_chains(boundaries, &chains, *parent))
            .collect();
    }

    chains.into_boxed_slice()
}

/// The chains of a boundary opened directly under `parent`.
fn parent_chains(
    boundaries: &[ScopeBoundary],
    chains: &[Chains],
    parent: ScopeParent,
) -> Vec<Box<[ScopeId]>> {
    let ScopeParent::Boundary(id) = parent else {
        return vec![Box::from([Singleton::ID])];
    };
    let index = find_index(boundaries, &id).expect("parent existence is validated before chains");

    chains[index]
        .iter()
        .map(|chain| std::iter::once(id).chain(chain.iter().copied()).collect())
        .collect()
}

/// The IDs present in every chain, sorted for binary search.
fn common_ids(chains: &[Box<[ScopeId]>]) -> Box<[ScopeId]> {
    let Some((first, rest)) = chains.split_first() else {
        return Box::new([]);
    };
    let mut common: Vec<ScopeId> = first
        .iter()
        .copied()
        .filter(|id| rest.iter().all(|chain| chain.contains(id)))
        .collect();

    common.sort();

    common.into_boxed_slice()
}
