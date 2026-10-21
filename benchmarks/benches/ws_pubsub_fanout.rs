//! WebSocket pub/sub fan-out — the three publish paths, across subscriber counts.
//!
//! `Publisher`/`TopicBus` delegate to the protocol-neutral [`SubscriptionRegistry`], which is what
//! this bench drives directly (it is generic over the frame type, so no protocol wiring is needed):
//!
//! - `emit` → `publish_frames`: fire-and-forget, `try_send`.
//! - `publish` → `deliver_frames::<16>`: backpressured, up to 16 subscribers awaited concurrently.
//! - `publish_to::<128>` → `deliver_frames::<128>`: backpressured, fully concurrent fan-out.
//!
//! Each iteration publishes one frame to every subscriber and then **drains every receiver in the
//! same thread** before the next iteration. Draining synchronously (rather than via spawned tasks)
//! keeps the queues empty deterministically, so no frame is ever dropped or backpressured by a lagging
//! consumer, and no detached task survives into the next case to contaminate its samples. The drain is
//! identical across all three paths, so the figure is publish-plus-consume for N subscribers and the
//! difference between the three isolates the send mechanism (`try_send` vs buffered `send().await`).

use std::cell::RefCell;
use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use upwell_axum::SubscriptionRegistry;

const DESTINATION: &str = "/bench/topic";
const FRAME_BYTES: usize = 64;
const CHANNEL_CAPACITY: usize = 64;
const SUBSCRIBER_COUNTS: [usize; 3] = [1, 16, 128];

type Frame = Vec<u8>;

/// A registry with `subscribers` subscriptions to one destination, plus the receiver halves so the
/// bench can drain them itself each iteration.
fn setup(subscribers: usize) -> (Arc<SubscriptionRegistry<Frame>>, Vec<mpsc::Receiver<Frame>>) {
    let registry = Arc::new(SubscriptionRegistry::<Frame>::new());
    let mut receivers = Vec::with_capacity(subscribers);

    for i in 0..subscribers {
        let (tx, rx) = mpsc::channel::<Frame>(CHANNEL_CAPACITY);
        let conn = registry.register();

        registry.subscribe(conn, &format!("sub-{i}"), DESTINATION, tx);
        receivers.push(rx);
    }

    (registry, receivers)
}

/// Drains every buffered frame from every receiver, so the queues are empty for the next publish.
fn drain(receivers: &mut [mpsc::Receiver<Frame>]) {
    for rx in receivers.iter_mut() {
        while rx.try_recv().is_ok() {}
    }
}

/// `emit`'s engine: synchronous fire-and-forget fan-out. No runtime needed — `try_send`/`try_recv`
/// do not require an executor.
fn emit_fanout(c: &mut Criterion) {
    let frame = vec![0u8; FRAME_BYTES];

    let mut group = c.benchmark_group("ws_emit_publish_frames");

    for subscribers in SUBSCRIBER_COUNTS {
        let (registry, mut receivers) = setup(subscribers);

        group.bench_function(BenchmarkId::from_parameter(subscribers), |bencher| {
            bencher.iter(|| {
                registry.publish_frames(DESTINATION, |_sub_id, _msg_id| black_box(frame.clone()));
                drain(&mut receivers);
            });
        });
    }

    group.finish();
}

/// `publish`'s engine: backpressured fan-out, 16 subscribers awaited concurrently.
fn publish_fanout(c: &mut Criterion) {
    let runtime = Runtime::new().expect("tokio runtime");
    let frame = vec![0u8; FRAME_BYTES];

    let mut group = c.benchmark_group("ws_publish_deliver_16");

    for subscribers in SUBSCRIBER_COUNTS {
        let (registry, receivers) = setup(subscribers);
        // `RefCell` (borrowed only for the synchronous drain, never across an await) lets the async
        // routine reach the receivers without the future escaping the `FnMut` with a mutable borrow.
        let receivers = RefCell::new(receivers);

        group.bench_function(BenchmarkId::from_parameter(subscribers), |bencher| {
            bencher.to_async(&runtime).iter(|| async {
                registry
                    .deliver_frames::<16>(DESTINATION, |_sub_id, _msg_id| black_box(frame.clone()))
                    .await;
                drain(&mut receivers.borrow_mut());
            });
        });
    }

    group.finish();
}

/// `publish_to::<128>`'s engine: backpressured fan-out, fully concurrent.
fn publish_to_fanout(c: &mut Criterion) {
    let runtime = Runtime::new().expect("tokio runtime");
    let frame = vec![0u8; FRAME_BYTES];

    let mut group = c.benchmark_group("ws_publish_to_deliver_128");

    for subscribers in SUBSCRIBER_COUNTS {
        let (registry, receivers) = setup(subscribers);
        let receivers = RefCell::new(receivers);

        group.bench_function(BenchmarkId::from_parameter(subscribers), |bencher| {
            bencher.to_async(&runtime).iter(|| async {
                registry
                    .deliver_frames::<128>(DESTINATION, |_sub_id, _msg_id| black_box(frame.clone()))
                    .await;
                drain(&mut receivers.borrow_mut());
            });
        });
    }

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(4))
        .sample_size(80);
    targets = emit_fanout, publish_fanout, publish_to_fanout
}
criterion_main!(benches);
