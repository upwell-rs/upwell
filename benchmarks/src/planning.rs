//! Registries with many entries into one scope, for measuring how preparing a generation
//! scales with the entries it plans.
//!
//! The scope holds the [`scoped`](crate::scoped) roster. Each entry injects one roster
//! component eagerly and one lazily, so the roster yields `MAX_SCOPED²` distinct target sets.
//! [`Signatures::Unique`] gives every entry its own target set while the pool lasts, and
//! [`Signatures::Shared`] reuses a handful, as an application whose handlers inject the
//! same few services does.

use std::sync::Arc;

use upwell_app::{
    AppRegistry, EntryDescriptor, PreparedScopeTopology, ScopeBoundary, ScopeParent, ScopeTopology,
};
use upwell_core::{DependencyDescriptor, EntryKey, Scope, ScopeId};
use upwell_di::{FromContainer, Lazy};

use crate::scoped::{self, CallScoped, MAX_SCOPED};

/// The entry counts the planning bench measures, a decade apart.
pub const ENTRIES: [usize; 3] = [100, 1_000, 10_000];

/// The number of target sets [`Signatures::Shared`] cycles through.
const SHARED: usize = 16;

/// The scope every planned entry opens.
pub struct Planned;

impl Scope for Planned {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "bench/planned");
    const RANK: u8 = 50;
    const NAME: &'static str = "Planned";
}

static BOUNDARIES: [ScopeBoundary; 1] = [ScopeBoundary::new::<Planned>(&[ScopeParent::Root])];

/// How many distinct target sets the entries of a registry use.
#[derive(Clone, Copy)]
pub enum Signatures {
    /// Every entry injects its own target set, until the roster's pool is exhausted.
    Unique,
    /// Entries cycle through a few target sets, so most of them share a plan.
    Shared,
}

impl Signatures {
    /// The bench label of this case.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unique => "unique",
            Self::Shared => "shared",
        }
    }
}

/// The targets of an entry injecting roster component `A` eagerly and `B` lazily.
fn targets<const A: usize, const B: usize>() -> Vec<DependencyDescriptor> {
    vec![
        <Arc<CallScoped<A>> as FromContainer>::dependency(),
        <Lazy<Arc<CallScoped<B>>> as FromContainer>::dependency(),
    ]
}

/// Every `targets::<A, B>` for `A` in the outer list and `B` in the inner one.
macro_rules! target_pool {
    ([$($a:literal),*], $inner:tt) => {
        [$(target_pool!(@row $a, $inner)),*]
    };
    (@row $a:literal, [$($b:literal),*]) => {
        [$(targets::<$a, $b> as fn() -> Vec<DependencyDescriptor>),*]
    };
}

/// Every distinct target set of the roster, `MAX_SCOPED` rows of `MAX_SCOPED`.
fn pool() -> [[fn() -> Vec<DependencyDescriptor>; MAX_SCOPED]; MAX_SCOPED] {
    target_pool!(
        [
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
            24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45,
            46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63
        ],
        [
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
            24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45,
            46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63
        ]
    )
}

/// The prepared topology of the [`Planned`] scope under the root.
pub fn topology() -> PreparedScopeTopology {
    ScopeTopology::new(&BOUNDARIES)
        .prepare()
        .expect("the planning topology validates")
}

/// A registry holding the whole roster in [`Planned`] and `count` entries into it, with
/// `signatures` distinct target sets. Entry keys are leaked, as the registry borrows them for
/// the program's lifetime.
pub fn registry(count: usize, signatures: Signatures) -> AppRegistry {
    let pool: Vec<_> = pool().into_iter().flatten().collect();
    let distinct = match signatures {
        Signatures::Unique => pool.len(),
        Signatures::Shared => SHARED,
    };
    let entries = (0..count)
        .map(|index| {
            let key = Box::leak(format!("bench/planned-{index}").into_boxed_str());

            EntryDescriptor {
                key: EntryKey::new(key).expect("bench entry keys are valid"),
                scope: Planned::KEY,
                targets: pool[index % distinct],
                owner: None,
                source: None,
            }
        })
        .collect();

    AppRegistry {
        components: scoped::roster(Planned::KEY).to_vec(),
        entries,
        ..AppRegistry::default()
    }
}
