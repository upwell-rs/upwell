# Conditional config and graph commit (#206) Implementation Record

> **Completion status:** Implemented on `feat/app/206/transactional-config-graph-commit-v2`. Every checklist item below records completed work rather than remaining implementation work.

**Goal:** Make a configuration change that alters component eligibility commit its configuration and effective runtime graph through one serialized, transactional reload path.

**Architecture:** `AppRuntime::reload_config` holds the `ConfigReloader` serialization lock and the runtime-transition writer in that order. It stages an isolated, generation-local candidate `ConfigStore`, derives condition facts, evaluates and validates a candidate graph, constructs its root against that candidate store, and builds a generation-local `HookManager` seeded into the candidate root. The candidate runtime generation is the authoritative publication; legacy `Cfg<T>` slots are committed synchronously afterwards for compatibility while both locks remain held.

**Tech Stack:** Rust 2024, tokio, arc-swap; crates: `upwell-config`, `upwell-app`, `upwell-di`, `upwell-core`, `upwell-hooks`.

**Spec:** https://github.com/upwell-rs/upwell/issues/206

## Completed Contract

- `ConfigReloader::stage()` prepares `StagedReload` without changing active state. It produces changed bindings, staged values, committable live-slot swaps, the re-read config tree, and a fresh candidate `ConfigStore` containing every bound configuration value.
- Candidate construction resolves configuration from that generation-local store. Its `Cfg` cells do not alias the active generation's cells, so factories and hooks observe only candidate values before publication.
- `ConditionFacts` remains a standalone trait extending `ConfigProperties`; configuration types explicitly implement it. `ConditionFactSource` stores the descriptors and erased scalar extraction, and explicit builder registration supplies sources to the application catalog. The abandoned approach of adding condition-fact methods to `ConfigProperties` is not part of the implementation.
- Every `RuntimeGeneration` carries `AppConditionState`: the immutable application catalog and the evaluation used to build that generation. Reload incrementally evaluates staged facts against the base generation's condition state.
- Each runtime generation owns a generation-local `HookManager`. Reload derives its descriptors from the candidate graph and seeds that manager into the candidate root; it does not reuse the active generation's manager.
- The config-reload and runtime-transition serializers are both held for the transaction. Lock order is config-reload serialization first, then the runtime-transition writer; a legacy config-only reload cannot interleave with a transactional reload.
- `PreparedRuntimeCommit` validates the candidate against the exact pinned base while the sole writer is held. It owns the stamped candidate and commit token, leaving no recoverable terminal publication failure.
- Terminal order is fixed: publish the authoritative candidate runtime generation first; then synchronously commit the legacy live config slots while both locks remain held; then release the locks. Runtime generation state is authoritative. Compatibility `Cfg<T>` slots are atomic per slot only, not atomically consistent across separately held handles.
- Before the commit token is consumed, every recoverable error, hook rejection, failed candidate factory, and cancellation leaves the active runtime generation and live configuration unchanged. A callback panic after publication is outside this rollback guarantee and is an internal-contract failure, not a recoverable transaction rejection.
- An unchanged source is a true no-op: no condition evaluation, graph planning, candidate construction, hook execution, root replacement, config-generation advance, or runtime-generation publication occurs.
- `RuntimeReloadReport` records `runtime_generation`, `config_generation`, `changed`, `hooks`, and `published`.
- Direct `ConfigReloader::reload()` remains config-only compatibility behavior. Its reload, watch, and signal trigger migration to transactional graph reload is explicitly deferred to #211.

## File Layout

- Transactional entry point: `crates/app/src/runtime/reload/mod.rs`.
- Reload tests: sibling directory `crates/app/src/runtime/reload/tests/`, rooted by `tests/mod.rs` with focused modules for no-op, publication, hook rejection, candidate failure, cancellation, concurrency, and provider switching.
- Runtime generation and commit token: `crates/app/src/runtime/generation.rs` with tests in `crates/app/src/runtime/generation/tests.rs`.
- Staged config reload and candidate store: `crates/config/src/managed/reload.rs`.
- Condition-fact trait and registration support: `crates/config/src/managed/mod.rs` and the application registry/builder integration.
- App error integration: `crates/app/src/error.rs`, where `ConfigReloadError` is boxed to remain within the crate-wide `result_large_err` budget.

## Completed Tasks

### Task 1: Stage config reloads

**Files:** `crates/config/src/managed/reload.rs`; config reload tests.

- [x] Extracted staged reload preparation from legacy reload behavior.
- [x] Added `StagedReload` access to changed bindings, staged values, candidate store, empty-state detection, hook execution, and synchronous live-slot commit.
- [x] Built the candidate `ConfigStore` from both changed and unchanged bindings, with fresh cells for the candidate generation.
- [x] Retained config-only compatibility semantics for direct `ConfigReloader::reload()`.
- [x] Verified candidate staging without publication.

### Task 2: Register and extract condition facts

**Files:** `crates/config/src/managed/mod.rs`; application registry and builder integration.

- [x] Added standalone `ConditionFacts` with explicit typed descriptors and scalar extraction.
- [x] Added explicit `ConditionFactSource` / builder registration rather than modifying `ConfigProperties`.
- [x] Extracted scalar snapshots from staged typed values, including supported erased-value shapes.
- [x] Preserved the macro-generated ergonomics follow-up for #208 without embedding it in #206.

### Task 3: Carry generation-local condition and hook state

**Files:** `crates/app/src/runtime/generation.rs`, runtime construction, and generation tests.

- [x] Added `AppConditionState` to each runtime generation and prepared generation.
- [x] Evaluated startup conditions through the same catalog/evaluation model used by reloads.
- [x] Added generation-local `HookManager` ownership and generation-pinned hook resolution.
- [x] Ensured candidate roots receive their own seeded hook manager.

### Task 4: Implement the transactional reload path

**Files:** `crates/app/src/runtime/reload/mod.rs`; runtime wiring; `crates/app/src/error.rs`.

- [x] Added `AppRuntime::reload_config()`.
- [x] Serialized staging, condition evaluation, graph planning, candidate factory construction, candidate hook execution, and terminal commit under both serializers.
- [x] Prepared `PreparedRuntimeCommit` against the exact base before token consumption.
- [x] Published the candidate runtime generation before committing compatibility live slots, synchronously under both locks.
- [x] Returned `RuntimeReloadReport { runtime_generation, config_generation, changed, hooks, published }`.
- [x] Boxed `ConfigReloadError` in the app error type.

### Task 5: Validate rollback, no-op, and serialization behavior

**Files:** `crates/app/src/runtime/reload/tests/`.

- [x] Verified that a candidate-staged factory reads staged configuration and not active live slots.
- [x] Verified a true no-op leaves both generations, root, construction, and hooks unchanged.
- [x] Verified hook rejection preserves the previous configuration and runtime generation.
- [x] Verified candidate factory failure preserves the previous configuration and runtime generation.
- [x] Verified cancellation releases both locks and does not block a subsequent reload.
- [x] Verified concurrent V2/V3 staging commits in serialized order without stale overwrite.
- [x] Verified primary-to-fallback and fallback-to-primary `Authenticator` provider transitions are accepted and resolve the effective provider transactionally.

### Task 6: Document compatibility and terminal semantics

**Files:** `crates/app/src/runtime/reload/mod.rs` and this implementation record.

- [x] Documented the two serializers and their lock order.
- [x] Documented runtime-first terminal publication and synchronous compatibility-slot commit.
- [x] Documented generation-pinned consistency versus per-slot legacy `Cfg<T>` observation limits.
- [x] Documented pre-publication rollback boundaries and the post-publication callback-panic internal contract.
- [x] Recorded direct `ConfigReloader` reload/watch/signal migration as #211 work.

## Pre-Implementation Notes Superseded by Completion

- The earlier proposal to make `ConfigProperties` expose condition-fact methods was replaced by standalone `ConditionFacts` plus explicit builder registration.
- The earlier proposed `crates/app/src/runtime/reload.rs` single-file layout was replaced by `crates/app/src/runtime/reload/mod.rs` and its sibling test directory.
- The earlier ambiguous commit ordering was resolved as runtime publication first, then synchronous legacy live-slot commit while both serializers remain held.
- The previous suggestion that legacy config triggers would be refactored as part of #206 was not implemented; they remain compatibility behavior until #211.

## Implementation Deviations and Completion Note

The completed implementation deliberately uses a candidate `ConfigStore` rather than exposing staged values only, so candidate factories and hooks have a coherent generation-local config view. It creates a fresh `HookManager` per generation and seeds it into the candidate root, rather than retaining or mutating the active manager. `PreparedRuntimeCommit` is the terminal commit token that fixes base validation and publication order. `ConfigReloadError` is boxed at the app boundary to satisfy the existing result-size constraint. These deviations are implemented behavior, not pending work.
