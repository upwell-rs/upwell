When you need to commit anything use `mise exec -- git <command>` in order to load the correct git profiles via mise.

## GitHub delivery workflow

- **PR ownership.** The project owner reviews, approves, and merges pull requests. Agents must never
  merge a pull request, even when CI and automated review are green.
- **Completion gate.** Do not consider a feature or issue complete until automated review explicitly
  reports that no issues remain.
- **Review conversations.** After addressing a review finding, resolve its GitHub review conversation.
- **Early tracking PRs.** When starting an epic, immediately create its branch and tracking pull
  request so progress and automated review are visible as work is added. Apply the same workflow to a
  `release/<version>` branch once it has a diff to its base branch.
- **Branch names.** Prefer `<branch-type>/<epic-or-module>/<issue-or-feature>/<optional-additional-path>`.
  Examples: `feat/app-cli/143/lifecycle` and `fix/app-macro/158/fallible-builder`.

## Before every commit / PR

- **Format.** Run `cargo fmt --all` before committing. A PR with unformatted code is blocked
  (`cargo fmt --all -- --check` must pass) — CI enforces it, so never push without it.
- **Lint.** `cargo clippy --workspace --all-targets --all-features` must be warning-free (this is
  the CI invocation — reproduce failures with it, not a per-crate clippy).

## Standard tests

- Run `just test` for the complete repository test policy: critical tests, extended tests, then
  doctests.
- Use `just test-critical` and `just test-extended` to run a single nextest cohort.
- Run doctests explicitly with `just test-doc`; nextest does not execute them.
- Run `just test-config` after changing nextest profiles or resource groups.
- Do not enable retries in repository test commands. Intermittent failures must remain visible.

## Test layout

Test modules always live in their own file, never inline in an impl file. For a module `foo`
(`foo.rs` or `foo/mod.rs`), declare `#[cfg(test)] mod tests;` in the module and put the tests in a
sibling `foo/tests.rs` (a `foo.rs` file may keep its `foo/tests.rs` submodule without becoming
`foo/mod.rs`). This keeps impl files clean and avoids the `clippy::items_after_test_module` lint.
Most modules with tests therefore gain a `<module>/tests.rs` file.

## Code organization

- Apply the boy-scout rule when touching code: improve nearby structure rather than adding to an
  already difficult file or function.
- Treat files around 400 lines as a strong decomposition signal, not a hard limit. Split large files
  into cohesive directory modules when practical.
- Keep functions small enough to understand independently. Extract reusable logical steps instead of
  extending long orchestration functions.
- Put substantial feature-gated concerns in dedicated cfg-gated modules rather than scattering many
  item-level `cfg` attributes through a large shared module.
- When an issue is discovered outside the immediate change, fix it if the change is low-risk and can
  be verified without regressions. Otherwise record a concrete, discoverable follow-up for future
  work rather than silently ignoring it.
