# Rust style and review criteria

This file is the review standard for Rust in this repository. CodeRabbit reads it through
`knowledge_base.code_guidelines` in `.coderabbit.yaml`, and contributors should follow it too.

Precedence: the owner rules in this file win. Where they differ from the Google Rust style guide,
the owner rule applies. Where this file is silent, rustfmt and the Rust API Guidelines apply.

## Owner formatting rules

- Put a blank line after every block statement (`if`, `for`, `match`, `while`, `loop`, closures
  with bodies), including before a `return`.
- Declare variables at the top of their scope, grouped together, then a blank line before the logic.
- Separate distinct logical groups inside a function with blank lines.
- Keep the `return` or trailing expression at the bottom, isolated by a blank line above it. Never
  chain it directly after a closing brace.
- Initialize at declaration. No late initialization without a reason.
- Run `cargo fmt --all`. Format is enforced by CI, so do not hand-format around rustfmt.

## Comments and docs

- No `//` comments in function bodies, struct fields, enum variants, or macro output.
- Allowed `//` comments: `SAFETY:` notes on `unsafe`, reasons for deliberately ignoring an error,
  and bug or odd-behavior notes. Bug or odd-behavior notes belong at function level.
- Every struct, enum, and trait has a brief `///` doc comment. Stateful widgets in Dart are the only
  exception and do not apply here.
- Public items use `///`. Modules and the crate root use `//!`. Keep docs short and do not restate
  what the code says. Document panics for inputs that can panic, and invariants for `unsafe` items.
- Omit variant docs when the variant name already says what it is.

## Tests

- Test modules live in their own file: `#[cfg(test)] mod tests;` in the module, with the tests in a
  sibling `<module>/tests.rs` that starts with `use super::*;`. Inline `mod tests { ... }` is not
  allowed in non-test files.
- Once a test file grows past roughly 400 lines, split it into `<module>/tests/mod.rs` (shared
  fixtures) and focused `<module>/tests/<topic>.rs` submodules.
- Prefer `expect("what was expected")` over `unwrap()`. Do not hoist panicking derivations above the
  assertions that explain the failure.
- Use a module-level `#![allow(dead_code)]` in test modules rather than contorting production code.
- Repeated test scaffolding is a defect. Extract shared helpers, or an internal `macro_rules!` when a
  function cannot express the pattern.
- Run tests with `cargo nextest run`, not `cargo test`. Doctests are the only exception.

## Modules and imports

- Name source files and leaf modules after their primary item.
- Non-leaf modules (`mod.rs` or `foo.rs` with `foo/`) hold only `mod` declarations, re-exports, and a
  little core glue. Implementation code belongs in a leaf module.
- Root modules re-export the public API from leaf submodules.
- Prefer directory modules. Do not mix `foo.rs` with `foo/*.rs`, except for `foo.rs` plus
  `foo/tests.rs`.
- One `use` statement imports from one module. Nested paths inside braces, such as
  `use std::{any::Any, sync::Arc}`, are not allowed. `use std::sync::{Arc, Mutex}` is allowed.
- Glob imports are allowed only for `use super::*` in test modules, `use Enum::*` in enum-focused
  code, and `pub use sub::*` in non-leaf modules. No preludes.
- Import traits locally inside the function that uses them, unless their methods are used throughout
  the file.
- Call top-level functions through their module (`mem::take(...)`). Import type names and use them
  unqualified.

## Functions and signatures

- Accept `&T`, `&[T]`, `&str`, `Option<&T>`. Reject `&Box<T>`, `&Vec<T>`, `&String`, `&Option<T>`.
- Library code returns iterators rather than `Vec<T>`. Application code may return `Vec<T>` when that
  is simpler.
- Avoid `impl Into<T>` parameters in application code. Use them only for library call-site ergonomics.
- Replace a `bool` parameter with an enum when the call site would be unclear.
- Keep functions small enough to read on their own. Extract reusable logical steps rather than
  growing an orchestration function.
- Boy-scout rule: when you touch a large file or function, split out cohesive modules or functions
  where practical. Around 400 lines is a strong signal to decompose.

## Ownership, borrowing, and cloning

- Prefer unique ownership and borrowing over `Rc` and `Arc`. Use shared ownership only when the
  alternative is unreasonably complex or an API requires it.
- Treat every `.clone()` as a smell. Borrow or move first. Keep a clone only for a genuine deep copy
  or when shared ownership or an API constraint needs it. Cloning `Arc` or `Rc` is exempt.
- Weigh the cost of cloning against borrowing when a value is on a hot path.

## Types and traits

- Use a newtype instead of a type alias when the type has its own meaning, its own API, may change,
  or is easy to confuse with another value.
- Prefer named fields over tuples. Tuples are fine for single-field newtypes and very local contexts.
- Strict typed fields over packed integers. Pack values only in serde. Use `bitflags` for flags.
- Use a Rust enum for every finite, compile-time-known set of choices.
- Use `#[derive(...)]` whenever it can express the impl. Avoid hand-written impls that derive could
  produce.
- Define a trait only for generic programming, `impl`/`dyn` use, or extension. Otherwise use inherent
  impls. Extension traits are suffixed `Ext`.
- Derive `Copy` for types that stay `Copy`, `Clone` for most types, and `Default` when there is a
  `new()` or a natural default state. Prefer `PartialEq` over `Eq` and `PartialOrd` over `Ord` unless a
  stronger guarantee is needed.
- Pair important `From` impls with a `new()` or `from_foo()` constructor.
- Trait item order, in traits and in every impl including macro output: associated types, then
  consts, then fns. Within each group, required items come before defaulted ones.
- Traits may declare `fn ... -> impl Future<Output = ...> + Send`. Impls use `async fn`.
- Prefer static generics (`T: Trait`, `impl Trait`) over `Box<dyn Trait>`. Users can box or enum-dispatch
  themselves if they need to.
- Wire values an older peer must keep, forward or report without understanding them (status and error
  codes, user-extensible sets) use an `http::StatusCode`-style newtype. Framework-owned sets, where an
  unknown value is an error or never crosses a version boundary, use a `#[non_exhaustive]` enum and
  reject unknown values when decoding. Never add an `Unknown(u8)` variant.
- Status and error codes are opaque to the framework. Do not add `category()` or `retryable()` methods.

## Errors and panics

- Library crates use strictly typed errors: `thiserror`, or a hand-written enum when the crate is small
  and must stay dependency-light.
- `anyhow` is for prototypes and binaries. Flag it in library crates, and flag it anywhere it hides
  the exact error a caller needs.
- Use `Option` for failures with no detail. Use `Result` when callers need context.
- Panic only for bugs and violated invariants. Panics are acceptable for test assertions and for
  non-fatal errors in developer-facing binaries.
- Prefer `match`, `if let`, or `let ... else` over check-then-`unwrap()`. Use `expect("reason")` when a
  panic is truly required.
- Assertion messages explain the expectation, and say whether a failure means API misuse or a bug.

## Concurrency and locks

- Keep locks at the lowest layer, for the shortest time, and never hold one across an unrelated
  `.await`.
- Prefer single ownership, split halves, channels, or a message to an owning task over a shared lock.
- For read-mostly values, prefer atomics for counters and flags, or snapshot swaps (`ArcSwap` and the
  `Live` pattern).
- For sync code use `std::sync::Mutex` or `std::sync::RwLock`. For async code use `tokio::sync` locks,
  and only when a guard must live across an `.await`.
- Use `RwLock` over `Mutex` only when reads clearly dominate. Otherwise prefer `Mutex`.
- Flag `parking_lot` or another lock crate unless the PR justifies it.
- Never reach inside a poisoned mutex.
- Avoid `Cell` and `RefCell` across threads. They are for single-thread interior mutability only.

## Macros

- Prefer functions, types, traits, and modules over macros. Before reaching for a proc macro, consider
  `macro_rules!`, and before that a plain code-generation step.
- Proc macros gate optional codegen with `cfg!(feature = "x")` inside the macro crate. Never emit
  `#[cfg(feature = ...)]` or `#[cfg_attr(feature = ...)]` into user code, because it resolves against
  the user's crate. `target_family = "wasm"` cfgs are fine.
- Keep the user's original token spans (`syn::Error::new_spanned`, stored key spans) so errors point
  at the offending token.
- Reuse user identifiers and paths verbatim so rust-analyzer hover and go-to-definition keep working.
- Path arguments such as `scope = ...` are raw `syn::Path` values. Do not normalize keywords.
- Document generated items directly, not through macro generation.

## Naming

- Follow std naming and the `as_`/`to_`/`into_` conventions with the matching `self` form.
- Constructors: `new()`, `new_with_foo()` when there are several, `try_new()` for fallible,
  `from_foo()` for value conversions paired with `From`.
- Getters `foo()`, mutable `foo_mut()`, take `take_foo()`, setters `set_foo()`, value-returning setters
  `replace_foo()`, closure accessors `with_foo()` and `with_mut_foo()`. Pair `len()` with `is_empty()`.
- No abbreviations outsiders will not know. Common Wikipedia abbreviations are fine.
- Descriptive lifetime names. Single-character lifetimes only for input-output ties, up to two.
- Short generic names: `T` and `U` when unbound.
- Framework-owned identifiers, such as namespaces and names, are declared as constants.
- Log lines use an explicit target: `target: "upwell::<area>"`.
- WebSocket terminology is strict. `Topic*` is pub/sub broadcast only. `Message*` is point-to-point to a
  `#[message]` handler. `Messaging*` is the shared protocol trait family. `PubSubProtocol` keeps its name.

## Public surface, unsafe, and features

- Add `#[non_exhaustive]` to public enums, and to public-field structs used across team boundaries.
- Add `#[must_use]` to functions whose results should not be ignored, to error types, and to lazy
  computations.
- Mark a public fn `const` only if it will stay `const`.
- Operator overloads only for their mathematical or logical purpose.
- `#[doc(hidden)]` items count as private unless the crate owner says otherwise. Do not rename them or
  change their visibility unless the API change was requested.
- No `unsafe` unless no safe implementation exists. Put it behind a safe abstraction that is isolated from
  business logic. Every `unsafe` block has a `SAFETY:` note.
- No unstable features. Do not run code before `main`. Prefer `linkme` for registration. The `inventory`
  backend exists as an intentional seam, so do not add new uses of it outside that seam.
- Do not call `catch_unwind` in first-party code.
- Do not add `openssl`, `openssl-sys`, or `rsa` to the dependency tree. Use rustls. Ask before adding
  any dependency.
- Keep ecosystem interfaces first-class: `tower::Layer`, `axum::middleware::from_fn`, and serde. Bespoke
  or DI paths are additive sugar, not replacements.
- Generalize internals for obvious future extensions, but defer the user-facing surface.

## Implementation habits

- Keep combinator chains short. Do not build them inside a `for` header. Avoid `for_each` unless it is
  justified.
- Use `format!` or `write!` for joining more than two strings or for literal substitution. Do not use
  `str::replace` for that.
- On `#[non_exhaustive]` enums, match every variant. Use `_` only for variants you do not care about.
- Annotate a type where a familiar reader cannot infer it. Prefer a binding annotation over a turbofish.
  State the type explicitly for `.into()`, `.parse()`, and complex iterator results.
- Upwell rule: model every finite, compile-time-known choice set as an enum.
- Upwell rule: never require ownership of `main`. Builder and lifecycle APIs stay available.
- Upwell rule: status and error codes stay opaque to the framework (see Types and traits).

## Repository conventions

- Every workspace crate has a `README.md` with Role, Usage, Internal role, and Feature flags. Add it in
  the same change that adds the crate.
- Keep new files and modules under the rules above, including tests and benchmarks.
- Do not hand-edit `CHANGELOG.md`. release-plz owns it, and commit messages are its input.

## Known pre-existing conflicts

These predate this file. Do not flag them in code a PR does not touch. Suggest a follow-up when a PR
touches them, and fix them when the change is low-risk (boy-scout rule).

- Some `mod.rs` files contain implementation code, which the non-leaf rule forbids.
- About 20 files use nested brace imports such as `use std::{any::Any, sync::Arc}`.
- The Google guide allows `//` non-doc comments. The owner rule forbids them in bodies. The owner rule
  applies, so only flag `//` comments the PR adds.
