use std::hint::black_box;
use std::sync::Arc;

use upwell_core::{Resolver, ResolverSet};

struct TestResolver;
impl Resolver for TestResolver {}

/// This is a deterministic complexity contract, not a wall-clock benchmark: request-time
/// cloning must remain an `Arc` bump even as benchmark timing varies between CI hosts.
#[test]
fn cloning_a_resolver_set_is_allocation_free() {
    const CLONES: usize = 4_096;

    let mut set = ResolverSet::new();
    set.insert(Arc::new(TestResolver));
    let mut clones = Vec::with_capacity(CLONES);

    let info = allocation_counter::measure(|| {
        for _ in 0..CLONES {
            clones.push(set.clone());
        }
    });

    black_box(clones);
    assert_eq!(
        info.count_total, 0,
        "ResolverSet::clone allocated on the hot path"
    );
}
