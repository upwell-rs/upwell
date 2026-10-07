//! Tests for the transactional config-and-graph reload entry point.
//!
//! Each test drives [`AppRuntime::reload_config`](crate::AppRuntime::reload_config)
//! over a file-backed app with one condition-gated component whose factory injects
//! `Cfg<T>` and whose `ConfigReload` hook records the proposed value, so the tests can
//! prove which store each stage of the transaction resolved through — and that every
//! failure or cancellation before the terminal commit leaves the complete previous
//! generation, config values included, active.

mod cancellation;
mod candidate_failure;
mod concurrency;
mod condition_panic;
mod fixture;
mod hook_rejection;
mod invalidation;
mod manager_consumer;
mod noop;
mod provider_switch;
mod publication;
mod reloader_manager;
