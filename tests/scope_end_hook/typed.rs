//! Hooks typed for one scope fire only for it, and next to the any-scope hook.

use upwell::ScopeEndKind;

use super::*;

/// The tags of the ends in `ends`, sorted.
fn tags(ends: &Mutex<Vec<ScopeEnd>>) -> Vec<u64> {
    let mut tags: Vec<u64> = ends
        .lock()
        .expect("the ends lock is healthy")
        .iter()
        .map(tag_of)
        .filter(|tag| (20..30).contains(tag))
        .collect();

    tags.sort_unstable();

    tags
}

#[tokio::test]
async fn a_typed_hook_fires_only_for_its_scope_and_next_to_the_any_hook() {
    let app = app("scope-end-typed").await;
    let siblings = app
        .runtime()
        .opener::<Sibling>()
        .expect("discovery declares the sibling scope");

    opener(&app)
        .builder(ended(20))
        .run(|| async {})
        .await
        .expect("the ended run succeeds");
    siblings
        .builder(Sibling { tag: 21 })
        .run(|| async {})
        .await
        .expect("the sibling run succeeds");

    assert_eq!(end_of(20).await.kind, ScopeEndKind::Completed);
    assert_eq!(end_of(21).await.kind, ScopeEndKind::Completed);

    for _ in 0..500 {
        if !tags(&TYPED_ENDED).is_empty() && !tags(&TYPED_SIBLING).is_empty() {
            break;
        }

        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    assert_eq!(tags(&TYPED_ENDED), [20]);
    assert_eq!(tags(&TYPED_SIBLING), [21]);
    assert_eq!(tags(&ENDS), [20, 21]);
}
