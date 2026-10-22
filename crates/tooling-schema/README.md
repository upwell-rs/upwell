# upwell-tooling-schema

> Versioned, protocol-neutral documents exchanged between Upwell applications and developer
> tooling.

Part of the [Upwell](../../README.md) framework, as the contract between `upwell-app`'s tooling
probe and [`cargo-upwell`](../cargo-upwell/README.md).

## Role

The schema has no dependency on application runtime state, Clap, Cargo metadata or a protocol. It
defines the `ToolingDocument` (generic resources, relationships, diagnostics, CLI metadata, facets,
and an optional what-if `TransitionPreview`), the `ProbeEnvelope` a probe publishes on success or
failure, identity validation, and the probe process contract: the hidden probe argument and the
`UPWELL_TOOLING_PROBE_*` environment variables, including the JSON `TransitionPreviewRequest` that
asks a probe to preview a config change.

## Usage

Applications never use it directly; the `upwell` facade's `tooling` feature re-exports it as
`upwell::tooling`. Tools decode envelopes with `ProbeEnvelope::from_json` and query the contained
document.

## Internal role

`upwell-app` projects prepared applications into these types; `cargo-upwell` builds its graph,
explanation and rendering queries on them. Within one major version, new fields are added only with
defaults.

## Feature flags

None.
