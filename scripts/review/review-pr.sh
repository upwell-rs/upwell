#!/usr/bin/env bash
# Usage: review-pr.sh <pr-number> <worktree> [model] [effort]
#
# Runs an independent, read-only Codex review of a PR checkout and posts it to the PR as a GitHub
# review with inline comments (resolvable threads). Requires `codex` (logged in), `gh` and `mise`.
# Output files (prompt, structured review, log) go to $UPWELL_REVIEW_OUT, default $TMPDIR/upwell-review.
set -euo pipefail

pr="$1"
worktree="$2"
model="${3:-gpt-6.1-terra}"
effort="${4:-medium}"
here="$(cd "$(dirname "$0")" && pwd)"
root="${UPWELL_REVIEW_OUT:-${TMPDIR:-/tmp}/upwell-review}"

mkdir -p "$root"
work="$(mktemp -d "$root/pr$pr.XXXXXX")"
out="$work/review.json"
log="$work/review.log"
prompt="$work/prompt.md"

head_sha="$(mise exec -- git -C "$worktree" rev-parse HEAD)"
base="$(mise exec -- gh pr view "$pr" --json baseRefName --jq .baseRefName)"
remote_head="$(mise exec -- gh pr view "$pr" --json headRefOid --jq .headRefOid)"
base_sha="$(mise exec -- gh pr view "$pr" --json baseRefOid --jq .baseRefOid)"

if [ "$head_sha" != "$remote_head" ]; then
    echo "worktree head $head_sha differs from the PR head $remote_head; refusing to review a stale tree" >&2
    exit 2
fi

if [ -n "$(mise exec -- git -C "$worktree" status --porcelain --untracked-files=no)" ]; then
    echo "worktree has uncommitted changes to tracked files; refusing to review something other than the PR" >&2
    exit 4
fi

mise exec -- git -C "$worktree" fetch -q origin "$base"

{
    cat "$here/review-prompt.md"
    printf '\nThis is PR #%s, head %s. Its base branch is origin/%s (a moving integration branch); review `git diff $(git merge-base origin/%s HEAD)...HEAD` in the working directory. Prioritise correctness, concurrency, layering and behaviour changes hidden in refactors over style nits.\n' "$pr" "$head_sha" "$base" "$base"
    printf '\nFINAL ANSWER FORMAT: reply with JSON only, matching the provided schema. `path` is repo-relative and `line` is a line number in the NEW version of the file that is part of the PR diff (prefer a changed line). Put a concrete failure scenario in `body`. Use verdict NO_FINDINGS with an empty findings list when you found nothing you could verify. `suggestion` may be an empty string. `unchecked` says what you could not check (for example, that you did not run the tests).\n'
} > "$prompt"

rm -f "$out"
codex exec -m "$model" -c "model_reasoning_effort=$effort" -s read-only --ephemeral \
    --output-schema "$here/review-schema.json" \
    -C "$worktree" -o "$out" - < "$prompt" > "$log" 2>&1

if ! python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); assert d["verdict"] in ("NO_FINDINGS","FINDINGS")' "$out"; then
    echo "review output is not valid structured JSON; not posting (see $log)" >&2
    exit 3
fi

current_head="$(mise exec -- gh pr view "$pr" --json headRefOid --jq .headRefOid)"
current_base="$(mise exec -- gh pr view "$pr" --json baseRefOid --jq .baseRefOid)"

if [ "$current_head" != "$head_sha" ]; then
    echo "the PR head moved to $current_head while the review ran; not posting a stale review (see $work)" >&2
    exit 5
fi

if [ "$current_base" != "$base_sha" ]; then
    echo "the PR base moved to $current_base while the review ran; not posting a review of a changed diff (see $work)" >&2
    exit 6
fi

python3 -I "$here/post_review.py" "$pr" "$head_sha" "$model" "$effort" "$out"
