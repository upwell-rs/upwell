//! Preparation cost over densely converging parent graphs.

use super::*;

/// A ladder rung: rungs `2k` and `2k + 1` form level `k`, each opened under both rungs of
/// level `k - 1`, so level `k` has `2^k` distinct opening chains.
struct Rung<const N: usize>;

macro_rules! rung_ids {
    ($($n:literal),* $(,)?) => {
        [$(
            match ScopeId::new(concat!("test/rung-", stringify!($n))) {
                Ok(id) => id,
                Err(_) => panic!("valid rung scope ID"),
            }
        ),*]
    };
}

const RUNG_IDS: [ScopeId; 80] = rung_ids!(
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49,
    50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73,
    74, 75, 76, 77, 78, 79
);

impl<const N: usize> Scope for Rung<N> {
    const ID: ScopeId = RUNG_IDS[N];
    const RANK: u8 = 200 - (N / 2) as u8;
    const NAME: &'static str = "Rung";
}

macro_rules! ladder {
    ($($level:literal),* $(,)?) => {
        [
            ScopeBoundary::new::<Rung<0>>(&[ScopeParent::Root]),
            ScopeBoundary::new::<Rung<1>>(&[ScopeParent::Root]),
            $(
                ScopeBoundary::new::<Rung<{ 2 * $level }>>(&[
                    ScopeParent::of::<Rung<{ 2 * $level - 2 }>>(),
                    ScopeParent::of::<Rung<{ 2 * $level - 1 }>>(),
                ]),
                ScopeBoundary::new::<Rung<{ 2 * $level + 1 }>>(&[
                    ScopeParent::of::<Rung<{ 2 * $level - 2 }>>(),
                    ScopeParent::of::<Rung<{ 2 * $level - 1 }>>(),
                ]),
            )*
        ]
    };
}

static LADDER: [ScopeBoundary; 80] = ladder!(
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39
);

#[test]
fn converging_parents_prepare_without_enumerating_chains() {
    let topology = prepare(&LADDER);
    let root = Singleton::ID;
    let top = Rung::<79>::ID;

    assert_eq!(topology.common_ancestors(&top), [root]);
    assert!(topology.is_reachable(&top, &root));
    assert!(!topology.is_reachable(&top, &Rung::<77>::ID));
    assert!(!topology.is_reachable(&top, &Rung::<0>::ID));
}
