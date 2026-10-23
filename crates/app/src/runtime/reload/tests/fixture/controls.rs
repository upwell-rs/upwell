//! Counters, recorded tokens and failure controls the probe factory and hooks honor,
//! so a test can observe each stage of a reload or inject a failure or cancellation.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub(crate) use crate::runtime::reload::tests::lock_test_guard;

pub(super) static FACTORY_CALLS: AtomicUsize = AtomicUsize::new(0);
pub(super) static HOOK_CALLS: AtomicUsize = AtomicUsize::new(0);
pub(super) static STAGE_ENTRIES: AtomicUsize = AtomicUsize::new(0);

pub(super) static PROPOSED_TOKENS: tokio::sync::Mutex<Vec<i64>> =
    tokio::sync::Mutex::const_new(Vec::new());
/// The `observed` token of the receiver each hook run resolved from its manager's
/// resolver context — the generation whose root the run resolved through.
pub(super) static RECEIVER_TOKENS: tokio::sync::Mutex<Vec<i64>> =
    tokio::sync::Mutex::const_new(Vec::new());
/// The process-local identity (`Arc::as_ptr`) of the receiver each hook run resolved
/// from its manager's resolver context, in run order.
pub(super) static RECEIVER_IDS: tokio::sync::Mutex<Vec<usize>> =
    tokio::sync::Mutex::const_new(Vec::new());
pub(super) static FACTORY_TOKENS: tokio::sync::Mutex<Vec<i64>> =
    tokio::sync::Mutex::const_new(Vec::new());
pub(super) static STAGED_TOKENS: std::sync::Mutex<Vec<i64>> = std::sync::Mutex::new(Vec::new());

/// The proposed token the config-reload hook rejects. `None` accepts every proposal.
pub(super) static HOOK_REJECT_TOKEN: tokio::sync::Mutex<Option<i64>> =
    tokio::sync::Mutex::const_new(None);

/// Whether the config-reload hook mutates its resolved receiver before returning, so a
/// test can prove a rejected candidate run leaves the active receiver untouched.
pub(super) static HOOK_MUTATES_RECEIVER: tokio::sync::Mutex<bool> =
    tokio::sync::Mutex::const_new(false);

/// The identity and token of the receiver each hook run actually mutated, recorded only
/// after the mutation is applied, in run order.
pub(super) static MUTATED_RECEIVERS: tokio::sync::Mutex<Vec<(usize, i64)>> =
    tokio::sync::Mutex::const_new(Vec::new());

/// The observed token of the receiver each [`ManagerProbeKind`] run resolved through
/// the fired manager's own resolver context, in run order.
pub(super) static MANAGER_PROBE_TOKENS: tokio::sync::Mutex<Vec<i64>> =
    tokio::sync::Mutex::const_new(Vec::new());

/// The process-local identity (`Arc::as_ptr`) of the receiver each
/// [`ManagerProbeKind`] run resolved, in run order.
pub(super) static MANAGER_PROBE_IDS: tokio::sync::Mutex<Vec<usize>> =
    tokio::sync::Mutex::const_new(Vec::new());

/// Whether the condition-fact scalar extraction panics when invoked, so a test can
/// prove the reload's condition stage is panic-contained before any candidate work.
/// Read synchronously inside the extraction callback, so it is a plain atomic.
pub(super) static SCALARS_PANIC: AtomicBool = AtomicBool::new(false);

/// Whether the condition-fact descriptor callback panics when invoked, so a test can
/// prove the reload's fact-descriptor collection is panic-contained. Read
/// synchronously inside the descriptor callback, so it is a plain atomic.
pub(super) static DESCRIPTORS_PANIC: AtomicBool = AtomicBool::new(false);

/// Whether the probe's config condition callback panics when evaluated, so a test can
/// prove the reload's incremental condition evaluation is panic-contained. Read
/// synchronously inside the callback, so it is a plain atomic.
pub(super) static CONDITION_PANIC: AtomicBool = AtomicBool::new(false);

/// Whether the candidate factory returns a DI error after resolving the staged value.
pub(super) static FACTORY_FAILS: tokio::sync::Mutex<bool> = tokio::sync::Mutex::const_new(false);

/// Whether the candidate factory parks on [`FACTORY_RELEASE`] after resolving the
/// staged value, so a test can cancel the reload mid-transaction.
pub(super) static FACTORY_WAITS: tokio::sync::Mutex<bool> = tokio::sync::Mutex::const_new(false);

/// Signaled by the factory once it is about to park on [`FACTORY_RELEASE`].
pub(super) static FACTORY_PARKED: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// The gate the factory parks on while the test decides whether to cancel the reload.
pub(super) static FACTORY_RELEASE: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Resets every counter and failure control. Call at the start of each test while
/// holding [`lock_test_guard`].
pub(crate) async fn reset_controls() {
    FACTORY_CALLS.store(0, Ordering::SeqCst);
    HOOK_CALLS.store(0, Ordering::SeqCst);
    STAGE_ENTRIES.store(0, Ordering::SeqCst);

    *PROPOSED_TOKENS.lock().await = Vec::new();
    *RECEIVER_TOKENS.lock().await = Vec::new();
    *RECEIVER_IDS.lock().await = Vec::new();
    *FACTORY_TOKENS.lock().await = Vec::new();
    STAGED_TOKENS
        .lock()
        .expect("staged token log is available")
        .clear();
    *HOOK_REJECT_TOKEN.lock().await = None;
    *HOOK_MUTATES_RECEIVER.lock().await = false;
    MUTATED_RECEIVERS.lock().await.clear();
    *MANAGER_PROBE_TOKENS.lock().await = Vec::new();
    *MANAGER_PROBE_IDS.lock().await = Vec::new();
    SCALARS_PANIC.store(false, Ordering::SeqCst);
    DESCRIPTORS_PANIC.store(false, Ordering::SeqCst);
    CONDITION_PANIC.store(false, Ordering::SeqCst);
    *FACTORY_FAILS.lock().await = false;
    *FACTORY_WAITS.lock().await = false;
}

pub(crate) fn factory_calls() -> usize {
    FACTORY_CALLS.load(Ordering::SeqCst)
}

/// Returns how many reload attempts entered config staging.
pub(crate) fn stage_entries() -> usize {
    STAGE_ENTRIES.load(Ordering::SeqCst)
}

pub(crate) fn hook_calls() -> usize {
    HOOK_CALLS.load(Ordering::SeqCst)
}

pub(crate) async fn proposed_tokens() -> Vec<i64> {
    PROPOSED_TOKENS.lock().await.clone()
}

/// The receiver token each hook run resolved, in run order.
pub(crate) async fn receiver_tokens() -> Vec<i64> {
    RECEIVER_TOKENS.lock().await.clone()
}

/// The process-local identity of the receiver each hook run resolved, in run order.
pub(crate) async fn receiver_ids() -> Vec<usize> {
    RECEIVER_IDS.lock().await.clone()
}

pub(crate) async fn factory_tokens() -> Vec<i64> {
    FACTORY_TOKENS.lock().await.clone()
}

/// Returns the candidate config tokens deserialized during reload staging.
pub(crate) fn staged_tokens() -> Vec<i64> {
    STAGED_TOKENS
        .lock()
        .expect("staged token log is available")
        .clone()
}

/// Makes the config-reload hook reject the proposal whose token is `token`.
pub(crate) async fn reject_hook_token(token: i64) {
    *HOOK_REJECT_TOKEN.lock().await = Some(token);
}

/// Makes the config-reload hook mutate its resolved receiver before returning.
pub(crate) async fn set_hook_mutates_receiver(mutates: bool) {
    *HOOK_MUTATES_RECEIVER.lock().await = mutates;
}

/// The identity and token of the receiver each hook run mutated, in run order.
pub(crate) async fn mutated_receivers() -> Vec<(usize, i64)> {
    MUTATED_RECEIVERS.lock().await.clone()
}

/// The receiver token each [`ManagerProbeKind`] run resolved, in run order.
pub(crate) async fn manager_probe_tokens() -> Vec<i64> {
    MANAGER_PROBE_TOKENS.lock().await.clone()
}

/// The process-local identity of the receiver each [`ManagerProbeKind`] run resolved,
/// in run order.
pub(crate) async fn manager_probe_ids() -> Vec<usize> {
    MANAGER_PROBE_IDS.lock().await.clone()
}

/// Makes the condition-fact scalar extraction panic when invoked.
pub(crate) fn panic_condition_scalars(panics: bool) {
    SCALARS_PANIC.store(panics, Ordering::SeqCst);
}

/// Makes the condition-fact descriptor callback panic when invoked.
pub(crate) fn panic_condition_descriptors(panics: bool) {
    DESCRIPTORS_PANIC.store(panics, Ordering::SeqCst);
}

/// Makes the probe's config condition callback panic when evaluated.
pub(crate) fn panic_condition_evaluation(panics: bool) {
    CONDITION_PANIC.store(panics, Ordering::SeqCst);
}

/// Makes the candidate factory fail with a DI error after resolving the staged value.
pub(crate) async fn fail_factory_after_staging(fails: bool) {
    *FACTORY_FAILS.lock().await = fails;
}

/// Makes the candidate factory park after resolving the staged value, until the reload
/// task is cancelled.
pub(crate) async fn park_factory_after_staging(waits: bool) {
    *FACTORY_WAITS.lock().await = waits;
}

/// Waits until the factory is parked inside candidate construction.
pub(crate) async fn factory_parked() {
    FACTORY_PARKED.notified().await;
}

/// Releases one factory blocked by [`park_factory_after_staging`].
pub(crate) fn release_factory() {
    FACTORY_RELEASE.notify_one();
}
