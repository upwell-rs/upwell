# Retained Singleton Generation Snapshots (#209) Implementation Plan

> **For agentic workers:** Use test-driven development and implement tasks in order.

**Goal:** Allow an isolated candidate root to share explicitly retainable unchanged singleton instances with the active generation while giving each generation independent DI slots and candidate-local provider projections.

**Architecture:** `ComponentDescriptor` carries a hidden typed generation-snapshot thunk. The thunk derives the component handle from active storage and re-boxes it into fresh generation-local storage (`Arc<T>` identity shared, `Live<T>` cell not shared). The app resolves `Retain` decisions from the pinned base root, applies explicit provenance policy to factoryless seeds, and keeps `RootResolver` generation-local.

**Spec:** #209, #203 architecture comment; prerequisite for #206 integration.

## Constraints

- Candidate roots never use the active root as parent or active `ComponentSource`.
- Providers are re-erased from candidate concrete slots using candidate selection/ordinals.
- Public user prebuilt instances remain unsupported and return `RestartRequired`.
- Framework-owned clone-safe seeds are explicitly marked shareable; ownership is never inferred from IDs.
- RootResolver is always recreated and attached to the candidate root.
- No hot update, static replacement factory, state export, or transition-local factory in this slice.

### Task 1: Typed component snapshot metadata

- Add hidden `generation_snapshot` metadata to `ComponentDescriptor`.
- Generated/typed descriptors use a generic adapter that calls `Injectable::from_stored` then `into_stored`.
- Plain `manual(...)` descriptors carry no adapter; add hidden typed `manual_of<T: Component>(...)` for framework seeds.
- Update macro-generated and handwritten descriptors.
- Tests: retained `Arc<T>` preserves `Arc::ptr_eq` but uses an independent `Live<T>` slot; by-value handles re-box correctly; mismatches are redacted errors.

### Task 2: Narrow active-root snapshot seam

- Add hidden `ScopeContainer::snapshot_singleton(descriptor)`.
- Validate singleton scope/type, invoke the adapter under panic containment, and validate output type.
- Never copy provider entries or expose `ScopeStore`.
- Tests: candidate provider aliases are rebuilt from retained concrete values under candidate selection; root ownership remains acyclic.

### Task 3: App provenance and retained candidate preparation

- Add private `SingletonSeedPolicy`/registry to application runtime assembly.
- Mark directories, `Dir<K>`, shutdown handle, hook manager, and config reloader framework-shareable; RootResolver runtime-bound; public `with_component` and protocol/manual seeds unsupported by default.
- Resolve plan `Retain` decisions directly from the pinned base root; remove unconditional retained-preparation rejection.
- Return `RestartRequired::ManualInstanceUnsupported` for unsupported manual seeds.
- Tests: mixed retain/reconstruct builds a complete root; framework builtins are present; user manual singleton requires restart; root-bound consumer behavior remains rejected.

### Task 4: Validation

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features`
- `just test`
- `cargo check --workspace --no-default-features`
- Automated review must report no issues before merge.
