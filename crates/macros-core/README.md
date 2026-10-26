# upwell-macros-core

> Shared codegen library for the Upwell proc-macros.

Part of the [Upwell](../../README.md) framework — the base codegen layer every macro crate builds on.

## Role

A proc-macro crate can only export proc-macros, so all the reusable parsing and code generation
lives here as an ordinary library. It owns the base expansions for `#[component]`, `#[config]`,
`#[methods]`, `#[injectable]`, `#[entry]`, the `#[when]` component conditions every component macro lowers, and
the named `app!` application macro, plus the building blocks the
expansions share: attribute parsing (`attr`), the extension seams (`extend` — `ParseKeyed`,
`ParseItem`, `ParseMethod`, `ComponentExt`), crate-path resolution (`paths::Paths`), field-injection
(`inject`), hooks (`hook`), the DI assertions (`di`), provider wiring (`provide`), client generation
(`client`), and the base impl-macro state machine (`methods`). These pieces are public so a plugin's
macro crate (e.g. `upwell-rpc-macros`) can reuse them to build its own macros without forking the
codegen. The crate is deliberately protocol-agnostic — protocol macro crates own their protocol
types via the extension seam.

## Usage

Most users depend on the [`upwell`](../../README.md) facade and never touch this crate. It is a
**codegen library consumed by macro crates, not by end users** — there are no proc-macros exported
here to attach to your code. You meet its output indirectly through `#[component]`, `#[config]`, and
the protocol macros.

Macro-crate authors reuse it directly. The extension seam is the main surface: implement
`ComponentExt` / `ParseMethod` / `ParseKeyed` to add protocol-specific keys and codegen, then call
`expand_component`, `methods::expand`, or the `run::<T, _>(item, expand)` parse-and-expand harness so
the macro turns errors into `compile_error!` instead of panicking. `Paths` selects the crate roots
the generated code refers to.

## Internal role

- `upwell-macros` is a thin proc-macro shim: each entry point forwards its token streams to the
  matching `component` / `config` / `methods` / `injectable` / `entry` / `when` / `app` function here.
- `upwell-rpc-macros` builds `#[service]`, `#[handlers]`, `#[rpc]` on top via the extension seam
  — reusing `expand_component`, `methods::expand`, `MethodArgs`, `Paths`, and `run`.

## Feature flags

| Feature | Effect |
|---|---|
| `client` | Emit the generated client (the `cfg!(feature = "client")` gate in client codegen). Forwarded from the macro crates' `client` feature. |
| `di-check` | Emit the compile-time dependency-injection assertions (read by `di::enabled()` and the field-injection codegen). Forwarded from the macro crates' `di-check` feature. |
