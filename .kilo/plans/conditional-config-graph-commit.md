# Conditional config and graph commit (#206) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Integrate conditional graph transitions with `ConfigReloader` as one serialized prepare → validate → hook → construct → commit transaction, so a config change that flips a component condition switches the effective graph transactionally.

**Architecture:** The `RuntimeTransitionCoordinator` (from #216) remains the sole serializer and publication owner. The config crate gains public *staged reload* primitives (`stage()` / `StagedReload::commit()`) so the app layer can drive one pipeline: begin transition → stage config → derive condition facts → evaluate candidate graph → plan transitions → prepare candidate root → run hooks → commit config slots and publish the runtime generation together. The existing `ConfigReloader::reload()` is refactored onto the same primitives and keeps its current config-only semantics until #211 migrates triggers.

**Tech Stack:** Rust 2024, tokio, arc-swap; crates: `upwell-config`, `upwell-app`, `upwell-di`, `upwell-core`.

**Spec:** https://github.com/upwell-rs/upwell/issues/206 (required sequence, failure/concurrency contract, acceptance criteria); epic boundary: PR #212 "Established v1 boundary" and "Governing invariants".

## Global Constraints

- One coordinator serializes every graph-changing trigger; there is no second reload pipeline (`RuntimeTransitionCoordinator`).
- All fallible and user-controlled work completes before publication; commit is one infallible publication.
- A proposal cannot commit unless its base generation is still current (`RuntimeTransition::publish` already enforces this).
- Failed, panicked, cancelled, or rejected preparation leaves the complete previous generation active — config slots included.
- Reports and traces contain stable IDs and categories, never raw current or expected config values.
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --all-features` must pass before every commit; run `just test` before pushing.
- Test modules live in sibling `tests.rs` files; never inline in impl files.
- Branch: `feat/app/206/transactional-config-graph-commit`, PR targets `feat/conditional-components/203`. Never merge locally; the project owner merges.

## Current-state facts (verified)

- `ConfigReloader::reload` (`crates/config/src/managed/reload.rs:359`): serialized by `inner.in_progress`; phase 1 re-reads/diffs/deserializes under the manager lock producing `prepared` slot swaps + `staged` values; phase 2 runs `ConfigReload` hooks; phase 3 commits swaps and `manager.adopt(new_root)`. No-op reloads advance a `u64` generation.
- `RuntimeTransitionCoordinator` (`crates/app/src/runtime/generation.rs:127`): `begin()` → `RuntimeTransition { attempt(), base(), finish_noop(), publish(PreparedRuntimeGeneration) }`; `publish` CAS-commits and stamps `base.id() + 1`; stale candidates rejected with `StaleRuntimeProposal`.
- `PreparedRuntimeGeneration::new(root, scopes, scope_plan, resolved, graph)`; `RuntimeGeneration` holds `graph: Arc<EffectiveGraph>` but **no condition-evaluation state**.
- `CandidateGraph::prepare_evaluation(base_generation, &AppRegistry, &AppConditionEvaluation, &PreparedScopeTopology)`, `resolve_transition(&active_graph, decisions)`, `ResolvedTransitionPlan::build_candidate_root(&candidate, retained, externals)` (`crates/app/src/transition.rs`).
- `AppRegistry::evaluate_conditions(facts, snapshot)` / `evaluate_changed_conditions(facts, previous, snapshot)` (`crates/app/src/registry.rs:99,114`) — **no production callers yet**; startup builds the graph from the full registry without condition evaluation.
- Condition facts: `ConfigFactDescriptor { id: ConfigFactId { config_type, binding_path, property_path }, kind, source }`; `ConditionFactSnapshot::new([(ConfigFactId, ConditionScalar)])`. **No mechanism exists to extract scalars from typed config values** — this slice introduces it (manual trait; macro ergonomics is #208).
- `App::build` (`crates/app/src/app.rs:609`) constructs the root, then `AppRuntime::new(...)`, and holds `reloader: ConfigReloader`.

## Non-goals (deferred by the epic)

- Proposal/decision hook kinds, canonical reports, reentrancy protection, and `ConfigReload` trigger migration — #211.
- Static replacement and transition-local/one-shot factories — #209 follow-ups.
- Macro-generated condition-fact extraction — #208 (this slice ships the manual trait).
- File-watch/signal trigger rewiring — documented as a migration path; triggers keep calling `ConfigReloader::reload()` until #211.

---

### Task 1: Staged reload primitives on `ConfigReloader`

**Files:**
- Modify: `crates/config/src/managed/reload.rs`
- Test: `crates/config/tests/config_reload.rs` (existing suite must stay green; add one new test)

**Interfaces:**
- Produces:
  - `pub struct StagedReload { changed: Vec<ChangedBinding>, staged: Vec<StagedConfig>, prepared: Vec<BindingSwap>, new_root: ConfigTree, hooks_run: bool }` (exact field types = whatever phase 1 of `reload()` already produces; `BindingSwap`/tree types are the existing private ones — the struct stays in `reload.rs` and only the type is made `pub`).
  - `impl ConfigReloader { pub fn stage(&self) -> Result<StagedReload, ConfigReloadError> }` — phase 1 only (re-read, diff, deserialize; panics converted to `ConfigReloadError::Panicked`).
  - `impl StagedReload { pub fn changed(&self) -> &[ChangedBinding]; pub fn staged(&self) -> &[StagedConfig]; pub fn commit(self) }` — `commit` performs the slot swaps and `manager.adopt(new_root)` under the manager lock (phase 3).
- Consumes: existing private phase-1/phase-3 code in `reload()`.

- [ ] **Step 1: Write the failing test** in `crates/config/tests/config_reload.rs`:

```rust
#[tokio::test]
async fn staged_reload_commits_only_when_explicitly_committed() {
    // Build a manager with one bound config type (reuse the file's existing
    // helper for a temp source file; mirror `reload_publishes_changed_bindings`).
    let reloader = /* same setup as the existing changed-binding test */;

    let staged = reloader.stage().expect("initial stage");
    assert!(staged.changed().is_empty(), "no changes before any edit");

    // Rewrite the source file with a changed value (same helper as existing tests).
    /* rewrite file */;

    let staged = reloader.stage().expect("stage after edit");
    assert_eq!(staged.changed().len(), 1);

    // Live value is untouched before commit.
    assert_eq!(live_value(&reloader), OLD_VALUE);

    staged.commit();
    assert_eq!(live_value(&reloader), NEW_VALUE);
}
```

- [ ] **Step 2: Run it** — `cargo nextest run -p upwell-config staged_reload_commits_only_when_explicitly_committed`. Expected: FAIL (`stage` does not exist).

- [ ] **Step 3: Implement** — extract phase 1 of `reload()` verbatim into `stage()`, phase 3 into `StagedReload::commit()`, and reimplement `reload()` as `stage()` → hooks (phase 2, unchanged) → `staged.commit()`. No behavior change to `reload()`.

- [ ] **Step 4: Run the full config suite** — `cargo nextest run -p upwell-config`. Expected: all pass, including the new test.

- [ ] **Step 5: Commit** — `feat(config): expose staged reload primitives for transactional graph commits`.

### Task 2: Condition facts from typed config values

**Files:**
- Modify: `crates/config/src/managed/mod.rs` and `crates/config/src/managed/reload.rs`
- Modify: `crates/app/src/registry.rs` and `crates/app/src/app.rs`
- Test: `crates/config/src/managed/tests.rs`, `crates/app/src/registry.rs`, `tests/config_macro.rs`

**Interfaces:**
- Produces:
  - `pub trait ConditionFacts: ConfigProperties { fn condition_facts() -> Vec<ConfigFactDescriptor>; fn condition_scalars(&self) -> Vec<(ConfigFactId, ConditionScalar)>; }` with explicit per-type implementations. The trait is separate because `#[config]` already emits `ConfigProperties`, so macro config types cannot override methods on that impl.
  - `ConditionFactSource::of::<T: ConditionFacts>(path)` captures descriptors and an erased scalar thunk. `AppBuilder::condition_facts::<T>(path)` registers that source explicitly; macro registration remains deferred to #208.
  - `AppRegistry::condition_snapshot(values)` extracts scalars from staged `(TypeId, path, value)` tuples. The thunk accepts both staging representations: plain `T` for changed bindings and `Arc<T>` for unchanged bindings.
- Consumes: `ConfigFactDescriptor`, `ConditionScalar`, `ConditionFactSnapshot` from `upwell-core`/`upwell-di`.

- [ ] **Step 1: Write the failing test** (config crate):

```rust
#[derive(serde::Deserialize, Default)]
struct FeatureFlags {
    enabled: bool,
    level: i64,
}

impl ConfigProperties for FeatureFlags { const NAME: &'static str = "FeatureFlags"; }

impl ConditionFacts for FeatureFlags {
    fn condition_facts() -> Vec<ConfigFactDescriptor> {
        vec![
            ConfigFactDescriptor { id: ConfigFactId::new("FeatureFlags", "flags", "enabled"), kind: ConditionScalarKind::Bool, source: descriptor_source!() },
            ConfigFactDescriptor { id: ConfigFactId::new("FeatureFlags", "flags", "level"), kind: ConditionScalarKind::Integer, source: descriptor_source!() },
        ]
    }

    fn condition_scalars(&self) -> Vec<(ConfigFactId, ConditionScalar)> {
        vec![
            (ConfigFactId::new("FeatureFlags", "flags", "enabled"), ConditionScalar::Bool(self.enabled)),
            (ConfigFactId::new("FeatureFlags", "flags", "level"), ConditionScalar::Integer(self.level as i128)),
        ]
    }
}

#[test]
fn condition_facts_extract_scalars_from_bound_values() {
    // Bind FeatureFlags at "flags", load, and assert the extracted snapshot
    // contains Bool(enabled) and Integer(level) under the right fact ids.
}
```

- [ ] **Step 2: Run it** — expected FAIL (`ConditionFacts` undefined).

- [x] **Step 3: Implement.** Keep `ConfigBinding` unchanged. Capture type-erased extraction in an explicit `ConditionFactSource`, register sources separately on `AppRegistry`, and add accessors to `StagedConfig` for the transactional app layer.

- [x] **Step 4: Run** config, macro, and app registry tests plus workspace clippy. Expected: PASS.

- [ ] **Step 5: Commit** — `feat(config): extract condition facts from typed config values`.

### Task 3: Retain condition-evaluation state in the runtime generation

**Files:**
- Modify: `crates/app/src/runtime/generation.rs` (`RuntimeGeneration`, `PreparedRuntimeGeneration`)
- Modify: `crates/app/src/runtime/mod.rs` (`AppRuntime::new` signature if needed)
- Modify: `crates/app/src/app.rs` (`PreparedApp::build` — evaluate initial conditions)
- Test: `crates/app/src/runtime/generation/tests.rs`

**Interfaces:**
- Produces: `RuntimeGeneration.condition: Arc<AppConditionState>` where `pub struct AppConditionState { facts: Vec<ConfigFactDescriptor>, evaluation: AppConditionEvaluation }` (exact shape: whatever lets `evaluate_changed_conditions` run against the committed generation). `PreparedRuntimeGeneration::new` gains the state parameter; the initial build evaluates conditions from the startup config snapshot with the same `evaluate_conditions` path reloads use (acceptance criterion: identical semantics).
- Consumes: Task 2's fact extraction; `AppRegistry::evaluate_conditions`.

- [ ] **Step 1: Write the failing test** in `crates/app/src/runtime/generation/tests.rs`: a prepared generation built with a non-empty `AppConditionState` exposes it through `RuntimeView` (`view.condition()`), and the initial generation's evaluation matches a direct `evaluate_conditions` call.

- [ ] **Step 2: Run** — expected FAIL.

- [ ] **Step 3: Implement** — thread the state through `PreparedRuntimeGeneration::new` and `commit`; update `AppRuntime::new` and `PreparedApp::build` (evaluate at startup from the initial config values; an app with no condition facts gets an empty-facts evaluation, which is a valid no-op catalog).

- [ ] **Step 4: Run** `cargo nextest run -p upwell-app`. Expected: PASS.

- [ ] **Step 5: Commit** — `feat(app): retain condition evaluation state per runtime generation`.

### Task 4: The transactional reload entry point

**Files:**
- Create: `crates/app/src/runtime/reload.rs` (+ `crates/app/src/runtime/reload/tests.rs`)
- Modify: `crates/app/src/runtime/mod.rs`, `crates/app/src/app.rs` (wire the reloader + registry + topology into the runtime), `crates/app/src/error.rs` (new error variants)

**Interfaces:**
- Consumes: Task 1 `stage()`/`commit()`, Task 2 snapshot, Task 3 generation state, `RuntimeTransitionCoordinator::begin/publish`, `CandidateGraph::prepare_evaluation`/`resolve_transition`/`build_candidate_root`.
- Produces:
  - `impl AppRuntime { pub async fn reload_config(&self) -> Result<ConfigReloadReport, crate::Error> }` (report type reused from config crate, or a new app-level report carrying generation + changed bindings + hook outcomes + transition summary).
  - Sequence inside one `transitions.begin()` lease: `reloader.stage()` → no-changes shortcut (`finish_noop`) → build `ConditionFactSnapshot` from staged values → `evaluate_changed_conditions(facts, &base.condition.evaluation, &snapshot)` → `CandidateGraph::prepare_evaluation(base.id(), ...)` → `resolve_transition(&base.effective_graph, [])` (v1: no explicit decisions; `validate_decision` defaults drive `RestartRequired` for unsupported cases) → `build_candidate_root(&candidate, Vec::new(), externals)` → run `ConfigReload` hooks (same filtering as `reload()`) → `staged.commit()` then `transition.publish(PreparedRuntimeGeneration::new(root, scopes, scope_plan, resolved, candidate_graph))`.
  - Failure contract: any `Err` before publish → drop the transition lease and the staged reload; nothing published, config untouched. `publish` failure (`StaleRuntimeProposal`) → report stale; config slots must **not** be committed before publish succeeds — commit order is `publish` first, then `staged.commit()`, with the writer lease still held so no other transition can interleave (document this ordering and its multi-slot observation limit).

- [ ] **Step 1: Write the failing test** — an app with one conditional component (condition on a bound config fact) built with the fact false; flip the config source; `runtime.reload_config()` succeeds; `runtime.view().root()` resolves the newly eligible component and `runtime.generation()` advanced; the old component is gone from the new generation's resolved set.
- [ ] **Step 2: Run** — expected FAIL (`reload_config` undefined).
- [ ] **Step 3: Implement** the sequence above, smallest correct version.
- [ ] **Step 4: Run** `cargo nextest run -p upwell-app`. Expected: PASS.
- [ ] **Step 5: Commit** — `feat(app): transactional config and graph reload through the transition coordinator`.

### Task 5: Failure, no-op, and concurrency contract tests

**Files:**
- Test: `crates/app/src/runtime/reload/tests.rs`

- [ ] **Step 1: Write tests** (each one first, watch it fail where it exercises new behavior):
  - `failed_dependent_reconstruction_retains_old_config_and_graph`: candidate factory fails → `reload_config` errors → old generation id unchanged, old config value still live, old component still resolvable.
  - `rejected_hook_aborts_the_whole_reload`: a `ConfigReload` hook returns reject → nothing committed.
  - `no_change_reload_is_cheap_and_reports_consistently`: no diff → `finish_noop`, generation id unchanged, report says no changes.
  - `concurrent_reloads_cannot_commit_out_of_order`: two `reload_config()` futures raced → both complete, final generation reflects the last-committed staging, no panic, no stale overwrite (coordinator serialization).
  - `restart_required_leaves_previous_generation_active`: a transition needing an unsupported strategy → `RestartRequired` error, generation unchanged.
- [ ] **Step 2: Run each; implement only what the failures demand** (this task should be mostly tests against Task 4's implementation; fix bugs it exposes).
- [ ] **Step 3: Full suite** — `just test`. Expected: PASS.
- [ ] **Step 4: Commit** — `test(app): pin the transactional reload failure and concurrency contract`.

### Task 6: Acceptance test and documentation

**Files:**
- Test: `crates/app/tests/` (or the existing integration-test location for app-level acceptance; check `crates/app/tests/` layout first)
- Modify: `crates/app/src/runtime/reload.rs` (doc comments), `crates/app/src/runtime/mod.rs` (`view()` doc: replace the "until transactional config integration is added" note)

- [ ] **Step 1: Write the acceptance test** from issue #206: enabling and disabling a custom `Authenticator` trait provider through config switches the effective provider transactionally — two conditional components providing the same trait, condition on a config enum fact; flip; reload; resolve the trait and assert the new provider; flip back; assert the old one.
- [ ] **Step 2: Run** — expected PASS after Task 4 (if it fails, fix Task 4's implementation, not the test).
- [ ] **Step 3: Document** in `reload.rs` module docs: the commit ordering (generation publish before config slot commit, both under the writer lease), the multi-slot observation limit (a reader holding only a `Cfg<T>` handle may observe new config one publication before/after the graph switch; a generation-pinned view via `AppRuntime::view()` is the consistent read), and the `ConfigReload` migration path (watch/signal triggers still use `ConfigReloader::reload()` until #211 migrates them onto this entry point).
- [ ] **Step 4: Full validation** — `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features`, `just test`, `cargo check --workspace --no-default-features`.
- [ ] **Step 5: Commit** — `docs(app): document transactional reload semantics and ConfigReload migration`.

## Self-review notes

- Spec coverage: required sequence steps 1–8 map to Tasks 1 (staging), 2 (condition inputs), 3+4 (evaluate/validate/plan/prepare/commit), 4 (hooks inside the transaction), 4+6 (reports, retirement via generation drop), 5 (failure/concurrency), 6 (docs + acceptance). Acceptance criteria map to Tasks 4–6.
- Type consistency: `StagedReload`, `ConditionFacts`, `AppConditionState`, `reload_config` are each defined once and referenced by exact name in later tasks.
- Known risk: `publish`-before-`staged.commit()` ordering means a crash between the two leaves config slots uncommitted while the graph switched — acceptable for v1 because the writer lease serializes transitions and the process is going down anyway; documented in Task 6. If review rejects this, invert to commit-then-publish and accept the mirrored observation window; both orderings must be documented, only one implemented.
