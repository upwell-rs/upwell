# cargo-upwell

> Cargo subcommand for Upwell developer tooling: discovers, builds and probes an application
> target without serving it.

Part of the [Upwell](../../README.md) framework, as its developer tooling front end.

## Role

`cargo upwell` selects one Cargo package and binary target through `cargo_metadata`, builds it, and
runs its generated private tooling probe. The probe prepares the real application (registration,
config, conditions, validation, graph planning) and publishes a versioned tooling document without
constructing components, running hooks, or opening ports. Every command then works from that
document:

| Command | What it does |
|---|---|
| `check`, `doctor` | Validates the selected application and diagnoses selection, build and probe failures |
| `inspect`, `export` | Displays or emits the prepared tooling document |
| `graph` | Renders resources and relationships, filtered by family and direction |
| `explain` | Explains one resource: relationships, provider selection, diagnostics and CLI ownership |
| `init`, `templates` | Scaffolds projects from the built-in or user template catalog |
| `completions` | Shell registration and workspace-aware completion caches |

## Usage

```sh
cargo install --path crates/cargo-upwell
cargo upwell inspect -p my-app
cargo upwell explain 'dyn my_app::Authenticator' -p my-app
```

`explain` on a trait type, provider or providing component lists every provider mapping of the
trait, whether the effective graph includes it, the condition path deciding it, and the consumers
that resolve to it.

### What-if transition previews

`--set PATH=VALUE` (repeatable, later assignments win) asks the probe what a config change would do:

```sh
cargo upwell explain 'dyn my_app::Authenticator' -p my-app --set auth.mode=default
```

The probe merges the assignments above the application's config sources, stages the changed
bindings, re-evaluates conditions and resolves the transition plan the runtime reload would use. It
stops before construction: no factory, hook or trusted condition callback runs and nothing is
written. The output lists the changed bindings, the components whose eligibility flips, and every
affected component with its planned action, the strategy the runtime would apply (live rebind, hot
update, transition handler, replacement or reconstruction) and the dependency reason paths that
cause it. Like runtime transition reports, previews carry paths, type names and stable IDs, never
config values. `VALUE` is `true`/`false`, a number, a `"quoted string"`, or bare text.

## Internal role

The library half (`cargo_upwell`) exposes discovery, build, probe execution, and the
presentation-neutral graph and explanation queries over
[`upwell-tooling-schema`](../tooling-schema/README.md) documents. The binary adds the Clap
interface, built-in renderers and sandboxed WASM renderer support. The probe contract (hidden
argument, `UPWELL_TOOLING_PROBE_*` environment, response file) is defined by the schema crate and
implemented by `upwell-app`'s tooling module.

## Feature flags

None.
