//! Shared support for the Upwell benchmark suite.
//!
//! Six concerns the individual benches reuse:
//!
//! - [`alloc`] — a tracking global allocator recording allocation *count*, cumulative *bytes*, and
//!   *live* bytes, so a bench can measure heap traffic and a test can prove a hot path leaks
//!   nothing.
//! - [`measure`] — a custom Criterion measurement that reports allocated **bytes** instead of
//!   wall-clock time, turning "how much memory does this cost" into a first-class, trended metric.
//! - [`di`] — builders that stand up small/moderate/large DI graphs layered across several scopes,
//!   using only the public `upwell-di` API.
//! - [`graph`] — large factory-backed singleton graphs for measuring graph preparation.
//! - [`reload`] — file-backed applications over those graphs for measuring whole reload
//!   transitions.
//! - [`http`] — in-process raw axum and Upwell routers for measuring per-request cost.

pub mod alloc;
pub mod di;
pub mod graph;
pub mod http;
pub mod measure;
pub mod reload;
