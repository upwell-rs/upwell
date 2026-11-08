# Independent PR review (Codex, Gemini or Copilot)

CodeRabbit is not available on the free plan, so pull requests are reviewed by an independent
model through a local CLI. The reviewer runs on its own and posts a GitHub review with inline,
resolvable comments. The PR's author then reads the threads, fixes them, replies and resolves each
thread after pushing, the same flow CodeRabbit used.

```sh
just review <pr> <worktree> [model] [effort] [provider] [focus]
scripts/review/review-pr.sh <pr> <worktree> [model] [effort] [provider] [focus-file]
```

- `<worktree>` is a checkout of the PR head; the script refuses a tree that differs from the PR head.
- Defaults: provider `codex`, model `gpt-6.1-terra`, effort `medium`. Use `gpt-6.1-sol` at `high` for large or risky PRs.
- Cost rules (owner): never run `gpt-6-astra` or `gemini-4-argon-*` except as a single instance on a very small, focused slice (pass a focus file); never run several expensive models in parallel.
- Output (prompt, `pr.diff`, `drafts.jsonl`, structured review, log) lands in a fresh directory under `$UPWELL_REVIEW_OUT` (default `$TMPDIR/upwell-review`) per run.

## Providers

| Provider | CLI | Models | Effort |
| --- | --- | --- | --- |
| `codex` | `codex` | `gpt-6.1-terra`, `gpt-6.1-sol` | `-c model_reasoning_effort` |
| `gemini` | `agy` (Antigravity) | `agy models` | part of the model name |
| `copilot` | `copilot` (GitHub Copilot CLI) | Copilot model ids | `--reasoning-effort` |

- `codex`: `codex exec -m <model> -c model_reasoning_effort=<effort> -s read-only --ephemeral --output-schema review-schema.json -C <worktree> -o <out> -` with the prompt on stdin.
- `gemini`: the Antigravity CLI `agy`, run with the worktree as cwd:
  `agy --model <model> --mode plan --sandbox --output-format stream-json --json-schema review-schema.json --add-dir <worktree> --add-dir <run dir> -p "<prompt>"`.
  Models come from `agy models` (for example `gemini-4-argon-high`, `gemini-3.8-flash-low`); the effort is part of the model name, so the effort argument is ignored and the footer says so. `--dangerously-skip-permissions` is never used; a session-only `--settings` allow list (`mcp(blast-radius/find_symbol)` style rules) permits only the read-only blast-radius and codegraph tools, and the full answer text is saved as `response.md` in the run dir (the last JSON block of it is used when `structured_output` is missing): plan mode plus the sandbox means file edits and commands are not executed in headless mode (the reviewer reads with its viewer and search tools). The prompt is passed as the `-p` argument because `-p` does not read stdin. The structured answer comes from the final `structured_output` of the stream. An extra checklist is appended to the prompt for this provider.
- `copilot`: the GitHub Copilot CLI, run non-interactively with the worktree as `-C`:
  `copilot -p "<prompt>" --model <model> --reasoning-effort <effort> --output-format json --no-ask-user --disable-builtin-mcps --disable-client-telemetry --no-auto-update -C <worktree> --add-dir <run dir> --available-tools view glob rg --deny-tool=shell --deny-tool=write --deny-tool=memory --deny-tool=url`.
  Models are Copilot model ids (for example `gpt-6.1-sol`, `gpt-6.1-terra`, `claude-sonnet-5.5`, `grok-4.7`, `kimi-k3`, `mai-code-1.1-flash`); the effort is passed as `--reasoning-effort` (a model that does not support the level makes the run fail with exit 10). Strictly read-only: only the `view`, `glob` and `rg` tools are exposed, shell, write, memory and URL access are denied, and the built-in GitHub MCP server is disabled. `--allow-all-tools`, `--allow-all-paths`, `COPILOT_ALLOW_ALL`, `--share`, `--share-gist` and memory are never used. No MCP servers are configured for it (the blast-radius and codegraph tools are not available to this provider). There is no JSON-schema flag, so the last fenced json block (or last JSON object) of the final assistant message is used and validated against `review-schema.json`; the full final message is saved as `response.md`. The JSONL stream (`assistant.message_delta`, `assistant.message`, `result` events) is filtered for drafts like the other providers. The same extra checklist as for gemini is appended.
  Account: the CLI is logged in with a second GitHub account that is not the repo owner's. It never posts by itself; the script posts the review with the main `gh` account.
- Any other provider exits with code 8.

## Diff first, local drafts

The script writes `git diff <merge_base>...<head>` (fixed commit ids) to `pr.diff` in the run directory; the prompt tells the reviewer to read it first and open other files only to verify a finding.

The reviewer cannot write files, so it prints each suspected finding at once as a line
`UPWELL_FINDING_DRAFT {json}` (the schema finding fields plus `"status": "draft"`). `stream_drafts.py` filters the provider output and appends every such line, flushed per line, to `<run dir>/drafts.jsonl` and to `$UPWELL_REVIEW_OUT/drafts/pr<N>-<head8>.jsonl`. If the provider dies (usage limit, timeout, crash) the script exits 10 without posting, and prints where the drafts are and how many.

Inspect drafts with `python3 -I scripts/review/drafts.py <drafts.jsonl>` (path:line, severity, title, body). Drafts are unverified and are never posted automatically: verify each by hand against the code.

Only the final structured answer is posted, and only after it validates against `review-schema.json` and the freshness checks pass. The prompt requires that answer to hold only findings the reviewer re-verified. The review footer names provider, model and effort.

## Guards, locks and dry run

- Refuses a stale tree (2), a tree with uncommitted tracked changes (4), and does not post if the PR head (5) or base (6) moved or the local checkout changed (7) while the review ran.
- One instance per provider and model: a lock directory `$UPWELL_REVIEW_OUT/locks/<provider>--<model>` (removed on exit; a lock of a dead process is taken over) makes a second concurrent run refuse with exit 9.
- `UPWELL_REVIEW_DRY_RUN=1` runs everything except the post and prints the would-be review.

| Exit | Meaning |
| --- | --- |
| 0 | posted (or dry run completed) |
| 2 | worktree head differs from the PR head |
| 3 | final output is not valid against the schema |
| 4 | worktree has uncommitted tracked changes |
| 5 | PR head moved during the review |
| 6 | PR base moved during the review |
| 7 | local checkout changed during the review |
| 8 | unknown provider |
| 9 | same provider and model already running |
| 10 | provider run failed (drafts kept) |

## Files

- `review-pr.sh` builds the prompt, runs the provider read-only and calls the poster.
- `stream_drafts.py` tees the provider output to the log and collects drafts; `drafts.py` prints them.
- `validate_review.py` validates the final answer against the schema.
- `post_review.py` posts the findings as one review. A finding on a changed line becomes an inline comment; any other finding goes into the review body.
- `review-prompt.md` holds the review rules; `review-drafts-prompt.md` the draft protocol; `review-extra-checklist.md` the checklist for the gemini and copilot providers, with `review-gemini-checklist.md` and `review-copilot-checklist.md` adding their tool notes; `review-schema.json` the output schema.

Treat the result as a second pair of eyes, not an approval: a clean review means no verified findings,
and the reviewer does not run the tests.
