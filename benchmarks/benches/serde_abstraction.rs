//! Upwell's generic-over-serde abstraction layers versus calling serde directly.
//!
//! The framework never lets a handler touch a wire format: response bodies go through
//! `Responder`, stream items through `StreamEncode`/`StreamDecode`, HTTP bodies through `HttpBody`
//! (`Json`/`Form`), and pub/sub payloads through `TopicCodec`/`StompCodec`. Each is a thin generic
//! wrapper over `postcard`, `serde_json`, or `serde_urlencoded`. This bench pairs every wrapper with
//! the exact raw call it delegates to, so any regression that makes an abstraction *cost* something
//! shows up as a gap between the two lines. The expectation is that they are indistinguishable.

use std::hint::black_box;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use serde::{Deserialize, Serialize};
use upwell_axum::client::{Form, HttpBody, Json};
use upwell_axum_stomp::{JsonCodec, StompCodec};
use upwell_rpc::Responder;
use upwell_transport::{StreamDecode, StreamEncode};

/// A representative response payload: a couple of scalars and two collections, so serialization
/// does real work rather than measuring dispatch over a single field.
#[derive(Serialize, Deserialize, Clone)]
struct Payload {
    id: u64,
    name: String,
    tags: Vec<String>,
    values: Vec<i64>,
    active: bool,
}

fn payload() -> Payload {
    Payload {
        id: 42,
        name: "upwell-benchmark-payload".to_string(),
        tags: vec![
            "alpha".into(),
            "beta".into(),
            "gamma".into(),
            "delta".into(),
        ],
        values: (0..16).collect(),
        active: true,
    }
}

/// A flat struct for the urlencoded form path (urlencoded cannot represent nested collections).
#[derive(Serialize, Clone)]
struct FormData {
    account: u64,
    region: String,
    enabled: bool,
    retries: u32,
}

fn form_data() -> FormData {
    FormData {
        account: 7,
        region: "eu-north-1".to_string(),
        enabled: true,
        retries: 3,
    }
}

/// The RPC server response codec (`postcard`) versus the raw `postcard` call it wraps.
fn rpc_responder(c: &mut Criterion) {
    let value = payload();

    let mut group = c.benchmark_group("serde_rpc_responder_encode");

    group.bench_function("abstraction", |bencher| {
        bencher.iter(|| black_box(Responder::respond(black_box(&value))));
    });

    group.bench_function("raw_postcard", |bencher| {
        bencher.iter(|| black_box(postcard::to_allocvec(black_box(&value)).unwrap()));
    });

    group.finish();
}

/// The streaming per-item codec (`postcard`, blanket impls) versus raw `postcard`, both directions.
fn stream_codec(c: &mut Criterion) {
    let value = payload();
    let encoded = StreamEncode::encode(&value).unwrap();

    let mut encode = c.benchmark_group("serde_stream_encode");

    encode.bench_function("abstraction", |bencher| {
        bencher.iter(|| black_box(StreamEncode::encode(black_box(&value)).unwrap()));
    });

    encode.bench_function("raw_postcard", |bencher| {
        bencher.iter(|| black_box(postcard::to_allocvec(black_box(&value)).unwrap()));
    });

    encode.finish();

    let mut decode = c.benchmark_group("serde_stream_decode");

    decode.bench_function("abstraction", |bencher| {
        bencher.iter(|| black_box(<Payload as StreamDecode>::decode(black_box(&encoded)).unwrap()));
    });

    decode.bench_function("raw_postcard", |bencher| {
        bencher.iter(|| black_box(postcard::from_bytes::<Payload>(black_box(&encoded)).unwrap()));
    });

    decode.finish();
}

/// The HTTP client body codecs (`serde_json` / `serde_urlencoded`) versus their raw calls.
fn http_body(c: &mut Criterion) {
    let value = payload();
    let form = form_data();

    let mut json = c.benchmark_group("serde_http_json_encode");

    json.bench_function("abstraction", |bencher| {
        bencher.iter(|| black_box(Json(black_box(&value)).encode().unwrap()));
    });

    json.bench_function("raw_serde_json", |bencher| {
        bencher.iter(|| black_box(serde_json::to_vec(black_box(&value)).unwrap()));
    });

    json.finish();

    let mut urlencoded = c.benchmark_group("serde_http_form_encode");

    urlencoded.bench_function("abstraction", |bencher| {
        bencher.iter(|| black_box(Form(black_box(&form)).encode().unwrap()));
    });

    urlencoded.bench_function("raw_serde_urlencoded", |bencher| {
        bencher.iter(|| {
            black_box(
                serde_urlencoded::to_string(black_box(&form))
                    .map(String::into_bytes)
                    .unwrap(),
            )
        });
    });

    urlencoded.finish();
}

/// The pub/sub topic codec (`serde_json`, via `StompCodec`) versus raw `serde_json`, both directions.
fn stomp_codec(c: &mut Criterion) {
    let value = payload();
    let body = <JsonCodec as StompCodec>::encode(&value).unwrap();
    let encoded = serde_json::to_vec(&value).unwrap();

    let mut encode = c.benchmark_group("serde_stomp_encode");

    encode.bench_function("abstraction", |bencher| {
        bencher.iter(|| black_box(<JsonCodec as StompCodec>::encode(black_box(&value)).unwrap()));
    });

    encode.bench_function("raw_serde_json", |bencher| {
        bencher.iter(|| black_box(serde_json::to_vec(black_box(&value)).unwrap()));
    });

    encode.finish();

    let mut decode = c.benchmark_group("serde_stomp_decode");

    decode.bench_function("abstraction", |bencher| {
        bencher.iter(|| {
            black_box(
                <JsonCodec as StompCodec>::decode::<Payload>(black_box(body.clone())).unwrap(),
            )
        });
    });

    decode.bench_function("raw_serde_json", |bencher| {
        bencher.iter(|| black_box(serde_json::from_slice::<Payload>(black_box(&encoded)).unwrap()));
    });

    decode.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(4))
        .sample_size(100);
    targets = rpc_responder, stream_codec, http_body, stomp_codec
}
criterion_main!(benches);
