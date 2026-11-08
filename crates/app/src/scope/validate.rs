use thiserror::Error;
use upwell_core::{Scope, ScopeId, Singleton, Transient};

use super::{ScopeBoundary, ScopeParent};

/// What topology validation reads of a boundary, so a topology can be validated before its
/// parent lists are made `'static`.
pub(super) trait TopologyNode {
    /// The boundary's stable identity.
    fn id(&self) -> ScopeId;

    /// The boundary's lifetime rank.
    fn rank(&self) -> u8;

    /// Every parent the boundary opens under.
    fn parents(&self) -> &[ScopeParent];
}

impl TopologyNode for ScopeBoundary {
    fn id(&self) -> ScopeId {
        ScopeBoundary::id(self)
    }

    fn rank(&self) -> u8 {
        ScopeBoundary::rank(self)
    }

    fn parents(&self) -> &[ScopeParent] {
        ScopeBoundary::parents(self)
    }
}

/// Validates identity-sorted `boundaries`: unique, unreserved identities, existing distinct
/// parents, no cycles, and parents that outrank their children.
pub(super) fn validate_sorted<B: TopologyNode>(boundaries: &[B]) -> Result<(), ScopeTopologyError> {
    validate_ids(boundaries)?;
    validate_parents(boundaries)?;
    validate_cycles(boundaries)?;
    validate_parent_ranks(boundaries)
}

/// A structural failure in a protocol-owned scope topology declaration.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum ScopeTopologyError {
    /// The same stable identity appears more than once.
    #[error("scope boundary '{id}' is declared more than once")]
    DuplicateId {
        /// The duplicated stable identity.
        id: ScopeId,
    },
    /// A declaration attempts to use a framework-owned anchor identity.
    #[error("scope boundary '{id}' uses a reserved framework identity")]
    ReservedId {
        /// The reserved root or transient identity.
        id: ScopeId,
    },
    /// A boundary declares no parent to be opened under.
    #[error("scope boundary '{id}' declares no opening parent")]
    NoParents {
        /// The parentless boundary identity.
        id: ScopeId,
    },
    /// A boundary lists the same opening parent more than once.
    #[error("scope boundary '{id}' lists parent '{parent}' more than once")]
    DuplicateParent {
        /// The child boundary identity.
        id: ScopeId,
        /// The repeated parent identity.
        parent: ScopeId,
    },
    /// A boundary names itself as a parent.
    #[error("scope boundary '{id}' cannot be its own parent")]
    SelfParent {
        /// The self-parented boundary identity.
        id: ScopeId,
    },
    /// A boundary's parent is not declared by the topology.
    #[error("scope boundary '{id}' refers to missing parent '{parent}'")]
    MissingParent {
        /// The child boundary identity.
        id: ScopeId,
        /// The undeclared parent identity.
        parent: ScopeId,
    },
    /// An application scope names a child no boundary of the topology declares.
    #[error("scope '{id}' names child '{child}', which no scope boundary declares")]
    MissingChild {
        /// The application scope that names the child.
        id: ScopeId,
        /// The undeclared child identity.
        child: ScopeId,
    },
    /// Parent links contain a structural cycle.
    #[error("scope topology contains a parent cycle involving {members:?}")]
    Cycle {
        /// The cycle members in canonical parent-link order.
        members: Box<[ScopeId]>,
    },
    /// A parent is not strictly longer-lived than its child.
    #[error(
        "scope boundary '{child}' has rank {child_rank}, but parent '{parent}' has rank {parent_rank}; parent rank must be greater than child rank"
    )]
    InvalidParentRank {
        /// The child boundary identity.
        child: ScopeId,
        /// The child lifetime rank.
        child_rank: u8,
        /// The parent boundary or root identity.
        parent: ScopeId,
        /// The parent lifetime rank.
        parent_rank: u8,
    },
}

/// Visit state of one boundary during cycle detection.
#[derive(Clone, Copy, PartialEq)]
enum Visit {
    Unvisited,
    OnPath,
    Done,
}

fn validate_ids<B: TopologyNode>(boundaries: &[B]) -> Result<(), ScopeTopologyError> {
    let root = Singleton::ID;
    let transient = Transient::ID;

    for (index, boundary) in boundaries.iter().enumerate() {
        let id = boundary.id();

        if index > 0 && boundaries[index - 1].id() == id {
            return Err(ScopeTopologyError::DuplicateId { id });
        }

        if id == root || id == transient {
            return Err(ScopeTopologyError::ReservedId { id });
        }
    }

    Ok(())
}

fn validate_parents<B: TopologyNode>(boundaries: &[B]) -> Result<(), ScopeTopologyError> {
    for boundary in boundaries {
        let id = boundary.id();
        let parents = boundary.parents();

        if parents.is_empty() {
            return Err(ScopeTopologyError::NoParents { id });
        }

        for (index, parent) in parents.iter().enumerate() {
            validate_parent(boundaries, id, *parent)?;

            if parents[..index].contains(parent) {
                return Err(ScopeTopologyError::DuplicateParent {
                    id,
                    parent: parent.id(),
                });
            }
        }
    }

    Ok(())
}

fn validate_parent<B: TopologyNode>(
    boundaries: &[B],
    id: ScopeId,
    parent: ScopeParent,
) -> Result<(), ScopeTopologyError> {
    let ScopeParent::Boundary(parent) = parent else {
        return Ok(());
    };

    if id == parent {
        return Err(ScopeTopologyError::SelfParent { id });
    }

    if find_index(boundaries, &parent).is_none() {
        return Err(ScopeTopologyError::MissingParent { id, parent });
    }

    Ok(())
}

/// Rejects any cycle through parent links. Requires every parent to exist.
fn validate_cycles<B: TopologyNode>(boundaries: &[B]) -> Result<(), ScopeTopologyError> {
    let mut visits = vec![Visit::Unvisited; boundaries.len()];
    let mut path = Vec::new();

    for index in 0..boundaries.len() {
        visit(boundaries, index, &mut visits, &mut path)?;
    }

    Ok(())
}

fn visit<B: TopologyNode>(
    boundaries: &[B],
    index: usize,
    visits: &mut [Visit],
    path: &mut Vec<usize>,
) -> Result<(), ScopeTopologyError> {
    match visits[index] {
        Visit::Done => return Ok(()),
        Visit::OnPath => {
            let start = path
                .iter()
                .position(|candidate| *candidate == index)
                .expect("a boundary on the path is in the path");
            let members = path[start..]
                .iter()
                .map(|member| boundaries[*member].id())
                .collect();

            return Err(ScopeTopologyError::Cycle {
                members: canonical_cycle(members),
            });
        }
        Visit::Unvisited => {}
    }

    visits[index] = Visit::OnPath;
    path.push(index);

    for parent in boundaries[index].parents() {
        let ScopeParent::Boundary(parent) = parent else {
            continue;
        };
        let parent =
            find_index(boundaries, parent).expect("parent existence is validated before cycles");

        visit(boundaries, parent, visits, path)?;
    }

    path.pop();
    visits[index] = Visit::Done;

    Ok(())
}

fn validate_parent_ranks<B: TopologyNode>(boundaries: &[B]) -> Result<(), ScopeTopologyError> {
    for boundary in boundaries {
        for parent in boundary.parents() {
            let parent_rank = match parent {
                ScopeParent::Root => Singleton::RANK,
                ScopeParent::Boundary(parent) => {
                    let index = find_index(boundaries, parent)
                        .expect("parent existence is validated before rank ordering");

                    boundaries[index].rank()
                }
            };
            let child_rank = boundary.rank();

            if parent_rank <= child_rank {
                return Err(ScopeTopologyError::InvalidParentRank {
                    child: boundary.id(),
                    child_rank,
                    parent: parent.id(),
                    parent_rank,
                });
            }
        }
    }

    Ok(())
}

/// The index of the boundary with `id` in identity-sorted `boundaries`.
pub(super) fn find_index<B: TopologyNode>(boundaries: &[B], id: &ScopeId) -> Option<usize> {
    boundaries
        .binary_search_by_key(id, |boundary| boundary.id())
        .ok()
}

/// Rotates a cycle reached through child → parent links so it starts at its smallest ID.
fn canonical_cycle(mut members: Vec<ScopeId>) -> Box<[ScopeId]> {
    let start = members
        .iter()
        .enumerate()
        .min_by_key(|(_, id)| **id)
        .map(|(index, _)| index)
        .unwrap_or_default();

    members.rotate_left(start);

    members.into_boxed_slice()
}
