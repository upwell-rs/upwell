//! Final release of displaced generations: reported exactly once, only after the last
//! owner of the displaced root lets go, and never for the current generation.

use super::*;
use crate::test_support::{CapturedEvent, capture_events};

const RELEASED: &str = "displaced runtime generation released";

/// The `(generation, attempt)` of every release event, in emission order.
fn releases(events: &[CapturedEvent]) -> Vec<(u64, u64)> {
    events
        .iter()
        .filter(|event| {
            event.target == TRANSITION_TARGET && event.field("message") == Some(RELEASED)
        })
        .map(|event| (number(event, "generation"), number(event, "attempt")))
        .collect()
}

fn number(event: &CapturedEvent, name: &str) -> u64 {
    event
        .field(name)
        .and_then(|value| value.parse().ok())
        .expect("release event carries a numeric field")
}

fn displaced(runtime: &AppRuntime) -> Vec<(u64, u64)> {
    runtime
        .displaced_generations()
        .iter()
        .map(|displaced| (displaced.generation.get(), displaced.attempt.get()))
        .collect()
}

#[tokio::test]
async fn an_unpinned_displaced_generation_is_released_at_commit() {
    let (runtime, _) = build_runtime(HashMap::new()).await;

    let (committed, events) = capture_events(publish_next(&runtime)).await;

    assert_eq!(committed.id(), RuntimeGenerationId::new(1));
    assert_eq!(releases(&events), [(0, 1)]);
    assert!(displaced(&runtime).is_empty());
}

#[tokio::test]
async fn a_pinned_view_defers_release_until_it_drops() {
    let (runtime, _) = build_runtime(HashMap::new()).await;
    let pinned = runtime.view();

    let (_, committed) = capture_events(publish_next(&runtime)).await;
    let still_alive = displaced(&runtime);
    let ((), dropped) = capture_events(async move { drop(pinned) }).await;

    assert!(releases(&committed).is_empty());
    assert_eq!(still_alive, [(0, 1)]);
    assert_eq!(releases(&dropped), [(0, 1)]);
    assert!(displaced(&runtime).is_empty());
}

#[tokio::test]
async fn an_open_scope_defers_release_until_it_closes() {
    let (runtime, _) = build_runtime(HashMap::new()).await;
    let session = runtime
        .open_scope_from_root(Session::KEY, Vec::new())
        .await
        .expect("session opens");

    let (_, committed) = capture_events(publish_next(&runtime)).await;
    let still_alive = displaced(&runtime);
    let ((), closed) = capture_events(async move { drop(session) }).await;

    assert!(releases(&committed).is_empty());
    assert_eq!(still_alive, [(0, 1)]);
    assert_eq!(releases(&closed), [(0, 1)]);
}

#[tokio::test]
async fn a_directly_held_root_defers_release_until_it_drops() {
    let (runtime, _) = build_runtime(HashMap::new()).await;
    let root = runtime.root();

    let (_, committed) = capture_events(publish_next(&runtime)).await;
    let still_alive = displaced(&runtime);
    let ((), dropped) = capture_events(async move { drop(root) }).await;

    assert!(releases(&committed).is_empty());
    assert_eq!(still_alive, [(0, 1)]);
    assert_eq!(releases(&dropped), [(0, 1)]);
}

#[tokio::test]
async fn repeated_switches_release_every_displaced_generation_exactly_once() {
    const SWITCHES: u64 = 3;

    let (runtime, _) = build_runtime(HashMap::new()).await;
    let expected = (0..2 * SWITCHES)
        .map(|generation| (generation, generation + 1))
        .collect::<Vec<_>>();

    let (still_alive, events) = capture_events(async move {
        let mut pins = Vec::new();

        for _ in 0..SWITCHES {
            pins.push(runtime.view());
            publish_next(&runtime).await;
        }

        let still_alive = displaced(&runtime);

        drop(pins);

        for _ in 0..SWITCHES {
            publish_next(&runtime).await;
        }

        assert!(displaced(&runtime).is_empty());
        assert_eq!(runtime.generation().get(), 2 * SWITCHES);

        drop(runtime);

        still_alive
    })
    .await;

    assert_eq!(still_alive, [(0, 1), (1, 2), (2, 3)]);
    assert_eq!(releases(&events), expected);
}
