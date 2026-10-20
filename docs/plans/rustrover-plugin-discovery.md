# RustRover Plugin Discovery

Status: discovery only — nothing here is scheduled or implemented.
Related: #149 (tooling epic), `docs/plans/cargo-upwell-probe-discovery-execution.md`

## Goal

Assess how a RustRover plugin can display Upwell project information by consuming `cargo-upwell`
machine output, activating only when the opened Cargo workspace depends on a supported Upwell
version. The plugin must reuse the IDE's existing Cargo model and toolchain rather than re-running
its own Cargo discovery, and must never scrape terminal text or parse application source to
reconstruct framework state.

## Consumer Boundary

The probe plan already fixes the boundary: editor integrations share `cargo-upwell` selection,
build, execution, and validation behavior. The plugin is therefore a thin presentation layer:

- the IDE answers "is this an Upwell project, and which version?" from its in-memory Cargo model;
- `cargo upwell` answers "what is this application?" through versioned JSON;
- the plugin owns UI, caching, scheduling, cancellation, and mapping source locations to editors.

Prepared application state comes only from the probe envelope and tooling document. PSI is used
only to navigate to positions the document already reports, never to discover resources.

## Platform Facts

### Build And Packaging

- IntelliJ Platform Gradle Plugin 2.x provides `rustRover(version)` as a target platform.
- The Rust support plugin ID is `com.jetbrains.rust`, bundled in RustRover and installable from
  Marketplace (plugin 22407) into IntelliJ IDEA. Depend on it with `bundledPlugin(...)` for
  RustRover or `plugin("com.jetbrains.rust:<version>")` for IDEA, and declare
  `<depends>com.jetbrains.rust</depends>` in `plugin.xml`.
- `org.toml.lang` is a separate bundled plugin, needed only for `Cargo.toml` editor features.

### Rust Plugin API

The Rust plugin has been closed source since RustRover's introduction (2023). The IntelliJ SDK
documentation has no RustRover page and no Rust extension-point list. The `org.rust.*` classes
below are inferred from the archived open-source `intellij-rust` sources and confirmed in use
against current RustRover lines by third-party plugins, but they are not a published, stable API.

| Need | API |
|---|---|
| Cargo projects | `project.cargoProjects.allProjects` → `CargoProject.workspace: CargoWorkspace?` |
| Packages | `CargoWorkspace.packages`, `findPackageByName(name)` |
| Package facts | `Package.name`, `version`, `origin` (`WORKSPACE`/`DEPENDENCY`/…), `source`, `dependencies`, `targets`, `featureState`, `rootDirectory` |
| Binary targets | `Package.targets` filtered by `TargetKind` binary, with `requiredFeatures` |
| Model refresh events | `CargoProjectsService.CARGO_PROJECTS_TOPIC` / `CargoProjectsListener` |
| Cache invalidation | `project.rustPsiManager.rustStructureModificationTrackerInDependencies` |
| Toolchain | `project.rustSettings.toolchain` (configured `cargo`), `CargoCommandLine` |
| Navigation | `org.rust.lang.core.psi.*` (`RsFunction`, `RsStructItem`, …) |

### IntelliJ Platform Hooks

- `ToolWindowFactory.isApplicableAsync(project)` hides the tool window for non-Upwell projects;
  `manage(toolWindow, manager)` can toggle availability as the Cargo model changes.
- `EditorNotificationProvider` shows a `Cargo.toml` banner for actionable states (tool missing,
  incompatible version, `tooling` feature disabled).
- `ExternalAnnotator` or the Problems view maps tooling `Diagnostic.sources` to editor ranges.
- `TrustedProjects` gates probing: a probe builds the target (running build scripts and proc
  macros) and executes the binary in tooling mode.
- `KillableProcessHandler` with background progress for cancellable probe runs.

## Activation Gate

Evaluate three layers in order of cost and stop at the first failure. Each failure is a distinct,
displayable state rather than silent deactivation, except layer 1's "not an Upwell project".

### Layer 1: Static Dependency Check (No Process)

From the IDE Cargo model, for each workspace package:

1. find a direct dependency named `upwell` (facade) or `upwell-app` (core crate);
2. parse its `version` as semver and test it against the supported framework range;
3. check that the `tooling` feature is enabled on that dependency, since without it the binary has
   no probe entry and `cargo upwell` exits with `ProbeFailure` (5).

Edge cases:

- inside this repository `upwell` is itself a `WORKSPACE` package, not a `DEPENDENCY`;
- `cargo upwell init --upwell-path` produces path dependencies with no registry `source`;
- multiple workspace packages may pin different Upwell versions — evaluate per package.

Cache the result per project, invalidated by the Rust dependency modification tracker, and
re-evaluate on `CARGO_PROJECTS_TOPIC`. This is the pattern spring-rs uses (see Prior Art) and is
cheap enough for EDT-adjacent callers such as icons and annotators.

### Layer 2: Tool Check (Once Per Session And On Toolchain Change)

Run `cargo upwell --version` with `CARGO` set to the IDE-configured cargo (`cargo-upwell` already
honours `CARGO`). Compare the reported tool and tooling-schema versions with the dependency
version from layer 1. Missing or incompatible tools produce an install/upgrade notification.

### Layer 3: Probe (On Demand Or Debounced)

Invoke machine commands with explicit selection so multi-application workspaces never hit the
ambiguity path (exit 3):

```text
cargo upwell export  --format json --manifest-path <abs> -p <package> --bin <binary> [features]
cargo upwell graph   --format json ...
cargo upwell explain --format json ... <resource>
cargo upwell check   --format json ...
cargo upwell doctor  --format json ...
```

Packages and binaries come from `CargoWorkspace` targets, honouring `requiredFeatures` and the
IDE's feature selection. Stable exit codes (0 success, 1 validation, 2 misuse, 3 selection,
4 build, 5 probe, 6 operational) map directly to tool-window states.

## Presentation Surface (Candidate)

- Tool window: application selector, resource tree (components, providers, config bindings,
  hooks, lifecycle, plugins), graph view, CLI provenance.
- `Cargo.toml` banner for gate failures; `doctor` output as the remediation view.
- Diagnostics from `check` surfaced as annotations at `SourceLocation`s.
- Later: run configurations wrapping the generated CLI, gutter markers that link a declaration to
  its resource identity via `explain`.

## Prior Art: intellij-spring-rs

[intellij-spring-rs](https://github.com/ouywm/intellij-spring-rs) is the closest existing framework
plugin for RustRover (Kotlin, ~100 files). It differs from this plan in both gating and data
source:

- **Gating.** `SpringProjectDetector` walks `cargoProjects` workspace packages and their direct
  dependencies, matching names by prefix and even substring (`contains("spring")`). There is no
  version check. The result is cached against `rustStructureModificationTrackerInDependencies`.
- **Discovery.** Entirely static source analysis. `SpringRsUnifiedScanner` iterates every Rust
  file via `FileTypeIndex`, walks PSI for attributed functions/structs (`#[component]`,
  `#[derive(Service)]`, route attributes, `add_plugin(...)` calls), and infers types from
  `let` bindings and call return types. `RustConfigStructParser` parses
  `#[derive(Configurable)]` structs in dependency sources to drive TOML completion. Nothing is
  built or executed; there is no framework-provided descriptor.

Consequences for spring-rs: it works instantly without a build and inside dumb-mode-safe caches,
but it reimplements framework semantics in the IDE, cannot see macro- or plugin-generated
registrations, conditional components, replacements, or effective configuration, and drifts as
the framework evolves.

Upwell already rejects that approach (the probe plan excludes source and macro-expansion parsing):
the prepared application's own projection is authoritative. What to borrow from spring-rs is the
plumbing — Cargo-model detection, modification-tracker caching, per-platform-line source sets and
`sinceBuild`/`untilBuild` pinning — not the scanner.

## Gaps To Close In cargo-upwell First

1. **Machine-readable version query.** `--version` is human text. Add a JSON form (for example
   `cargo upwell version --format json`) reporting tool version, tooling-schema version, and the
   supported framework range, so the plugin asks the tool instead of hard-coding a range.
2. **0.x compatibility rule.** `schema_requirement()` accepts `>=X.Y.Z,<(X+1).0.0`, which for 0.x
   means a 0.20 consumer accepts a 0.27 envelope, while Cargo semver treats 0.x minor releases as
   breaking. Decide the supported-version definition before the plugin depends on it.
3. **Static applicability signal.** Consider exposing whether a dependency set can be probed
   (e.g. the required `tooling` feature) in a form the IDE model can check without building.

## Risks

- **Unstable API.** `org.rust.*` may change between platform lines. Pin `sinceBuild`/
  `untilBuild`, run the Plugin Verifier per line, and isolate Rust-plugin calls behind a small
  adapter (spring-rs keeps per-version source sets).
- **Test environment.** The bundled Rust plugin has been reported to fail to load in the
  test sandbox until its dependent modules are also declared as bundled plugins.
- **Probe cost.** Each probe is a full build into `<target>/upwell/build`, isolated from the IDE's
  build cache but duplicating compilation. Never probe on every model refresh; debounce on save or
  require explicit refresh, and cancel by killing the process tree.
- **Trust.** Probing executes project code; require a trusted project.

## Open Questions

- Is `featureState` on the dependency package reliable for the `tooling` check, or does RustRover
  resolve metadata with all features enabled?
- Should the plugin bundle or auto-install a matching `cargo-upwell`, or only recommend one?
- Which surfaces ship first: tool window only, or diagnostics as well?
- Is IntelliJ IDEA + Rust plugin an explicit support target alongside RustRover?

## Sources

- IntelliJ Platform Gradle Plugin 2.x and its dependencies extension (`rustRover(...)`):
  <https://plugins.jetbrains.com/docs/intellij/tools-intellij-platform-gradle-plugin.html>
- Archived `intellij-rust` sources (`CargoProjectService.kt`, `CargoWorkspace.kt`):
  <https://github.com/intellij-rust/intellij-rust>
- IntelliJ `ToolWindowFactory`:
  <https://github.com/JetBrains/intellij-community/blob/master/platform/platform-api/src/com/intellij/openapi/wm/ToolWindowFactory.kt>
- intellij-spring-rs: <https://github.com/ouywm/intellij-spring-rs>
- Bundled Rust plugin not loaded in tests:
  <https://platform.jetbrains.com/t/bundled-rust-bundled-plugin-not-loaded-in-test-environment/4011>
- Rust plugin on Marketplace: <https://plugins.jetbrains.com/plugin/22407-rust>
