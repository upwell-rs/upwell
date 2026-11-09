use axum::http::Extensions;

use super::*;

/// A seeded value.
#[derive(Clone, Debug, PartialEq)]
struct Tenant(&'static str);

#[test]
fn a_waiting_seed_is_read_into_the_opening_seeds() {
    let mut extensions = Extensions::new();

    extensions.insert(PendingSeed(Tenant("acme")));

    let seeds = collect(&[read_seed::<Tenant>, read_seed::<u8>], &extensions);

    assert_eq!(seeds.len(), 1, "only the waiting seed is read: {seeds:?}");
}

#[test]
fn no_waiting_seed_reads_nothing() {
    let seeds = collect(&[read_seed::<Tenant>], &Extensions::new());

    assert!(seeds.is_empty());
}
