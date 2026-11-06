# Independent PR review with Codex

CodeRabbit is not available on the free plan, so pull requests are reviewed by an independent
model through the Codex CLI. The reviewer runs on its own and posts a GitHub review with inline,
resolvable comments. The PR's author then reads the threads, fixes them, replies and resolves each
thread after pushing, the same flow CodeRabbit used.

```sh
just review <pr> <worktree> [model] [effort]
```

- `<worktree>` is a checkout of the PR head; the script refuses a tree that differs from the PR head.
- Defaults: `gpt-6.1-terra` at `medium` effort. Use `gpt-6.1-sol` at `high` for large or risky PRs.
  Keep `gpt-6-astra` for small, focused areas and only when plenty of usage is left.
- Output (prompt, structured review, log) lands in `$UPWELL_REVIEW_OUT`, default `$TMPDIR/upwell-review`.

Files:

- `review-pr.sh` builds the prompt, runs `codex exec` read-only with a JSON schema and calls the poster.
- `post_review.py` posts the findings as one review. A finding on a changed line becomes an inline
  comment; any other finding goes into the review body.
- `review-prompt.md` holds the review rules. Update it when the repository rules change.
- `review-schema.json` is the structured output schema.

Treat the result as a second pair of eyes, not an approval: a clean review means no verified findings,
and the reviewer does not run the tests.
