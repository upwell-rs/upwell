# Independent PR review (Codex, Gemini or Copilot)

CodeRabbit is not available on the free plan, so pull requests are reviewed by independent models
through local CLIs. The review runs in two stages and posts one GitHub review with inline,
resolvable comments. The PR's author then reads the threads, fixes them, replies and resolves each
thread after pushing, the same flow CodeRabbit used.

```sh
just review <pr> <worktree> [model] [effort] [provider] [focus] [verify_provider] [verify_model] [verify_effort]
scripts/review/review-pr.sh <pr> <worktree> [model] [effort] [provider] [focus-file] [verify-provider] [verify-model] [verify-effort]
```

- `<worktree>` is a checkout of the PR head; the script refuses a tree that differs from the PR head.
- `model`, `effort` and `provider` choose the discovery run. Defaults: provider `codex`, model `gpt-6.1-terra`, effort `medium`.
- The verifier defaults to the discovery provider, model and effort, each on its own. Override them with the positional `verify_*` arguments (an empty value means the default) or with `UPWELL_VERIFY_PROVIDER`, `UPWELL_VERIFY_MODEL` and `UPWELL_VERIFY_EFFORT`; a positional value wins over the variable. When you change the verifier's provider, set its model (and effort) too, since a discovery model id is rarely valid for another provider.
- Cost rules (owner): never run `gpt-6-astra` or `gemini-4-argon-*` except as a single instance on a very small, focused slice (pass a focus file); never run several expensive models in parallel.
- Output lands in a fresh run directory under `$UPWELL_REVIEW_OUT` (default `$TMPDIR/upwell-review`) per run; see [Run directory](#run-directory).

## Two stages

1. **Discovery** reads `pr.diff`, checks suspected defects against the code and prints every one as an `UPWELL_FINDING_DRAFT` line the moment it suspects it. It holds no final verification gate of its own: its final answer is only a done marker (`discovery-schema.json`: `status` `DISCOVERY_DONE`, the draft count, a summary of what it looked at and what it could not check). Drafts are saved to disk as they arrive and are never posted, so a run that dies half-way still leaves usable drafts.
2. **Verification** is a new provider process (codex `--ephemeral`, a fresh `agy -p` or `copilot -p` session; never a resumed session) that has not seen the discovery conversation. It gets only the drafts (framed as untrusted claims, with ids `D1`..`Dn`), the PR metadata (number, title, head, base, merge base), `pr.diff`, the review rules for judging severity, and the same read-only worktree access and tool allow lists as discovery. For each draft it answers `keep`, `modify` (corrected `severity`, `path`, `line`, `title`, `body`, `suggestion`) or `drop`, each with a reason (`verify-schema.json`). It may not add findings.

`verify_drafts.py apply` then validates the verifier's answer and refuses to post (exit 3) when a decision names an unknown or a repeated draft id, the answer breaks the schema, or a kept or modified finding targets a line that is not an added or context line of `pr.diff`. An answer that leaves out any draft id is refused too (exit 3, artifacts kept, and the `UPWELL_REVIEW_VERIFY_ONLY` resume command printed), so an omitted draft is never silently treated as dropped; an empty reason is recorded as "(no reason given)". A kept draft is posted verbatim (the verifier's echoed fields are ignored); a modified one uses the verifier's fields. Only this stage's output is posted.

With zero drafts, verification is skipped and a NO_FINDINGS review is posted, but only when discovery returned a valid done marker that reports zero drafts (otherwise exit 3 with the reason printed and the run dir kept: a run that answered in some other format may have put findings in its answer instead of drafts, and a marker counting drafts that were never saved means they were lost). A discovery answer that is not a valid done marker is only a warning when drafts exist. With drafts saved, a valid done marker that reports more drafts than were saved (`verify_drafts.py saved`) is refused too (exit 3, run dir kept, nothing verified): the drafts file can be resumed with `UPWELL_REVIEW_VERIFY_ONLY` after manual inspection.

The review footer names both stages: discovery provider, model and effort with the draft count, and verifier provider, model and effort with N kept, M modified and D dropped (or "skipped (no drafts)").

Before verification starts and again before posting, the head, base and local checkout checks run, so a moved PR does not cost a verification run.

### Cost and recommended mixes

Every review with drafts costs two provider runs. Verification is usually the cheaper one: it reads the diff and only the code around each draft, but a long drafts list or a large diff still adds up. Mixes that work:

- discovery `copilot` `kimi-k3` (or another cheap Copilot model) at `low`/`medium`, verification `codex` `gpt-6.1-terra` at `medium`;
- discovery `codex` `gpt-6.1-terra`, verification `codex` `gpt-6.1-sol` at `high` for large or risky PRs;
- one model for both (the default) when you just want the self-check without a second opinion.

A different model in each stage gives a real second opinion; the same model still helps because the verifier starts without the discovery's reasoning.

### Resume: verification only

When usage runs out after discovery (exit 10 after drafts were saved, or a lock or verification failure), verify the saved drafts without rediscovering:

```sh
UPWELL_REVIEW_VERIFY_ONLY=<drafts.jsonl> scripts/review/review-pr.sh <pr> <worktree> [model] [effort] [provider] '' [verify-provider] [verify-model] [verify-effort]
```

Every failure that keeps drafts prints this command ready to paste. The file may be a run's `drafts.jsonl`, the mirror `pr<N>-<head8>.jsonl` (all discovery runs of that head, duplicates are merged) or a run's `verify-input.jsonl`. Each saved draft carries the PR number and full head sha it was found on (stamped by `stream_drafts.py`; a model cannot set them); an unstamped legacy draft counts only when the file is named `pr<N>-<head8>.jsonl`. A draft of another PR or head is refused with exit 5, the same meaning as a head that moved. The stale-tree, dirty-tree and freshness guards apply as usual; the diff is recomputed for the current base. The footer says the drafts came from a saved file. An empty drafts file exits 1.

## Providers

| Provider | CLI | Models | Effort |
| --- | --- | --- | --- |
| `codex` | `codex` | `gpt-6.1-terra`, `gpt-6.1-sol` | `-c model_reasoning_effort` |
| `gemini` | `agy` (Antigravity) | `agy models` | part of the model name |
| `copilot` | `copilot` (GitHub Copilot CLI) | Copilot model ids | `--reasoning-effort` |

Each stage runs its provider the same way, with the stage's schema (`discovery-schema.json` or `verify-schema.json`, written as `<schema>` below) and prompt. The schema is also appended to the prompt, for the providers that have no schema flag.

- `codex`: `codex exec -m <model> -c model_reasoning_effort=<effort> -s read-only --ephemeral --output-schema <schema> -C <worktree> -o <answer> -` with the prompt on stdin.
- `gemini`: the Antigravity CLI `agy`, run with the worktree as cwd:
  `agy --model <model> --mode plan --sandbox --output-format stream-json --json-schema <schema> --add-dir <worktree> --add-dir <run dir> -p "<prompt>"`.
  Models come from `agy models` (for example `gemini-4-argon-high`, `gemini-3.8-flash-low`); the effort is part of the model name, so the effort argument is ignored and the footer says so. `--dangerously-skip-permissions` is never used; a session-only `--settings` allow list (`mcp(blast-radius/find_symbol)` style rules) permits only the read-only blast-radius and codegraph tools, and the full answer text is saved as `<stage>.response.md` in the run dir (the last JSON block of it is used when `structured_output` is missing): plan mode plus the sandbox means file edits and commands are not executed in headless mode (the reviewer reads with its viewer and search tools). The prompt is passed as the `-p` argument because `-p` does not read stdin. The structured answer comes from the final `structured_output` of the stream. Tool notes (`review-gemini-checklist.md`) are appended to both stages' prompts and the extra checklist to discovery's.
- `copilot`: the GitHub Copilot CLI, run non-interactively with the worktree as `-C`:
  `copilot -p "<prompt>" --model <model> --reasoning-effort <effort> --output-format json --no-ask-user --disable-builtin-mcps --disable-client-telemetry --no-auto-update -C <worktree> --add-dir <run dir> --available-tools view glob rg <read-only MCP tools> --deny-tool=shell --deny-tool=write --deny-tool=memory --deny-tool=url --disable-mcp-server MCP_DOCKER --disable-mcp-server playwright --disable-mcp-server railway`, run with the worktree as cwd.
  Models are Copilot model ids (for example `gpt-6.1-sol`, `gpt-6.1-terra`, `claude-sonnet-5.5`, `grok-4.7`, `kimi-k3`, `mai-code-1.1-flash`); the effort is passed as `--reasoning-effort` (a model that does not support the level makes the run fail with exit 10). Strictly read-only: only the `view`, `glob` and `rg` tools and the read-only code navigation tools of the user's MCP servers (`~/.copilot/mcp-config.json`, tool names `<server>-<tool>`) are exposed: `blast-radius-blast_radius`, `-find_symbol`, `-symbol_info`, `-call_slice`, `-module_graph`, `-dead_code`, `-diff_blast` and `codegraph-codegraph_explore`. The blast-radius plan/apply, `open_project`, `show` and `selection` tools are not exposed; the server indexes the workspace it starts in (the worktree, as the cwd), so `open_project` is not needed. codegraph answers only in a worktree that has a `.codegraph/` index. Shell, write, memory and URL access are denied, the built-in GitHub MCP server is disabled, and the MCP_DOCKER, playwright and railway servers are not started. `--allow-all-tools`, `--allow-all-paths`, `COPILOT_ALLOW_ALL`, `--share`, `--share-gist`, `--resume`, `--continue` and memory are never used. There is no JSON-schema flag, so the last fenced json block (or last JSON object) of the final assistant message is used and validated against the stage's schema; the full final message is saved as `<stage>.response.md`. The JSONL stream (`assistant.message_delta`, `assistant.message`, `result` events) is filtered for drafts like the other providers. Tool notes (`review-copilot-checklist.md`) are appended to both stages and the extra checklist to discovery.
  Account: the CLI is logged in with a second GitHub account that is not the repo owner's. It never posts by itself; the script posts the review with the main `gh` account.
- Any other provider (for either stage) exits with code 8.

## Drafts

The script writes `git diff <merge_base>...<head>` (fixed commit ids) to `pr.diff` in the run directory; both prompts tell the model to read it first and open other files only to check a draft.

The discovery model cannot write files, so it prints each suspected finding at once as a line
`UPWELL_FINDING_DRAFT {json}` (the schema finding fields plus `"status": "draft"`). `stream_drafts.py` filters the provider output and appends every such line, stamped with `pr`, `head`, `provider`, `model` and `effort` and flushed per line, to `<run dir>/drafts.jsonl` and to the mirror `$UPWELL_REVIEW_OUT/drafts/pr<N>-<head8>.jsonl`. Draft lines printed by the verifier are ignored. If a provider dies (usage limit, timeout, crash) the script exits 10 without posting and prints where the drafts are, how many, and the resume command.

Inspect drafts with `python3 -I scripts/review/drafts.py <drafts.jsonl>` (path:line, severity, title, body).

Each verification writes `verification.jsonl` to the run dir (one record per draft: `id`, `decision`, `reason`, the original `draft`, the posted `final` finding for keep/modify, the discovery `origin`, the `verifier` and the `run` directory) and appends the same records to `$UPWELL_REVIEW_OUT/drafts/pr<N>-<head8>.verification.jsonl`.

## Guards, locks and dry run

- Refuses a stale tree (2), a tree with uncommitted tracked changes (4), and does not verify or post if the PR head (5) or base (6) moved or the local checkout changed (7) while the review ran.
- One instance per provider and model: an exclusive `flock(2)` on `$UPWELL_REVIEW_OUT/locks/<provider>--<model>.lock`, held on a file descriptor, makes a second concurrent run refuse with exit 9 (the holder's pid is in `<lock>.pid`). The kernel releases it however the run ends, so a crashed run leaves no stale lock and no run ever deletes another's.
- Locks per stage: discovery takes its model's lock at the start. When the verifier is the same provider and model, that one lock covers both stages and is kept. Otherwise the discovery lock is released after discovery and the verifier's lock is taken (non-blocking) before verification; if the verifier model is busy the run exits 9 and prints the resume command, the drafts stay on disk. A run never holds two locks at once and never waits for one, so two runs with swapped models cannot deadlock. A verification-only run takes only the verifier's lock.
- `UPWELL_REVIEW_DRY_RUN=1` runs both stages and everything except the post, prints the would-be review body and inline comments and saves the payload as `payload.json` in the run dir.

| Exit | Meaning |
| --- | --- |
| 0 | posted (or dry run completed) |
| 1 | bad arguments (missing focus or drafts file, empty drafts file in verification-only mode) |
| 2 | worktree head differs from the PR head |
| 3 | an answer is not valid (schema, unknown, repeated or missing draft id, kept line outside the diff, no done marker without drafts, a done marker counting more drafts than were saved) |
| 4 | worktree has uncommitted tracked changes |
| 5 | PR head moved during the review, or the drafts belong to another PR or head |
| 6 | PR base moved during the review |
| 7 | local checkout changed during the review |
| 8 | unknown provider |
| 9 | same provider and model already running |
| 10 | a provider run failed (drafts kept) |

## Run directory

`pr.diff`; `discovery-prompt.md`, `discovery.log`, `discovery.json` (done marker), `discovery.response.md`; `drafts.jsonl` and `stamp.json`; `verify-input.jsonl` (deduplicated drafts with ids), `verify-prompt.md`, `verify.log`, `verify.json` (the raw verifier answer), `verify.response.md`; `verification.jsonl`; `review.json` (the review to post, `review-schema.json`) and `stages.json` (footer data); `payload.json` in a dry run. Files of a stage that did not run are absent.

## Files

- `review-pr.sh` runs the stages and the guards and calls the poster; `review-lib.sh` holds its provider invocation, lock and freshness functions.
- `stream_drafts.py` tees the provider output to the log and collects drafts; `drafts.py` prints them.
- `verify_drafts.py` prepares drafts for verification (head check, ids, prompt block) and turns the verifier's decisions into `review.json`, `verification.jsonl` and `stages.json`.
- `validate_review.py` validates an answer against a schema; `diffmap.py` maps diffs to the commentable new-file lines, decoding the paths git C-quotes (octal escapes, `\t`, `\"`, `\\`).
- `test_review_tools.py` holds fixture tests of the fail-closed checks and the path decoding: `python3 -I scripts/review/test_review_tools.py`.
- `post_review.py` posts the findings as one review. A finding on a changed line becomes an inline comment; any other finding goes into the review body.
- `review-prompt.md` holds the review rules; `review-drafts-prompt.md` the discovery protocol; `verify-prompt.md` the verification rules; `review-extra-checklist.md` the discovery checklist for the gemini and copilot providers, with `review-gemini-checklist.md` and `review-copilot-checklist.md` their tool notes; `discovery-schema.json`, `verify-schema.json` and `review-schema.json` the answer schemas.

Treat the result as a second pair of eyes, not an approval: a clean review means no verified findings,
and neither stage runs the tests.
