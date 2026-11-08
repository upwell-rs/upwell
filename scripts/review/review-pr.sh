#!/usr/bin/env bash
# Usage: review-pr.sh <pr-number> <worktree> [model] [effort] [provider] [focus-file]
#
# Runs an independent, read-only review of a PR checkout (provider codex, gemini or copilot) and posts it to the
# PR as a GitHub review with inline comments (resolvable threads). Requires `gh` and `mise` plus the
# provider CLI (`codex`, `agy` for gemini, or `copilot`).
# Output files (prompt, diff, drafts, structured review, log) go to $UPWELL_REVIEW_OUT, default
# $TMPDIR/upwell-review. UPWELL_REVIEW_DRY_RUN=1 runs everything except the post and prints a summary.
# Exit codes: 2 stale tree, 3 invalid output, 4 dirty tree, 5 head moved, 6 base moved, 7 checkout
# changed, 8 unknown provider, 9 model locked, 10 provider failed.
set -euo pipefail

pr="$1"
worktree="$2"
model="${3:-gpt-6.1-terra}"
effort="${4:-medium}"
provider="${5:-codex}"
focus_file="${6:-}"
here="$(cd "$(dirname "$0")" && pwd)"
root="${UPWELL_REVIEW_OUT:-${TMPDIR:-/tmp}/upwell-review}"

case "$provider" in
    codex | gemini | copilot) ;;
    *)
        echo "unknown provider '$provider'; expected codex, gemini or copilot" >&2
        exit 8
        ;;
esac

if [ "$provider" = "gemini" ]; then
    effort="set by the model name"
fi

if [ -n "$focus_file" ] && [ ! -f "$focus_file" ]; then
    echo "focus file $focus_file does not exist" >&2
    exit 1
fi

mkdir -p "$root/locks" "$root/drafts"

# Takes an exclusive flock(2) on the file $1 through fd 9, which this shell keeps open until it
# exits, and records this pid next to it for the refusal message. The kernel releases the lock
# however the process ends, so there is no stale lock to take over and no lock to delete. The
# long-running provider commands get fd 9 closed (9>&-) so an orphan of a killed run cannot keep
# holding it.
hold_lock() {
    exec 9>> "$1"

    python3 -I -c 'import fcntl; fcntl.flock(9, fcntl.LOCK_EX | fcntl.LOCK_NB)' 2> /dev/null || return 1
    echo "$$" > "$1.pid"
}

lock="$root/locks/$provider--$model.lock"

if ! hold_lock "$lock"; then
    holder="$(cat "$lock.pid" 2> /dev/null || true)"

    echo "another review with $provider/$model is already running (pid ${holder:-unknown}, lock $lock); run one instance at a time" >&2
    exit 9
fi

work="$(mktemp -d "$root/pr$pr.XXXXXX")"
out="$work/review.json"
log="$work/review.log"
prompt="$work/prompt.md"
diff_file="$work/pr.diff"
drafts="$work/drafts.jsonl"

head_sha="$(mise exec -- git -C "$worktree" rev-parse HEAD)"
base="$(mise exec -- gh pr view "$pr" --json baseRefName --jq .baseRefName)"
remote_head="$(mise exec -- gh pr view "$pr" --json headRefOid --jq .headRefOid)"
base_sha="$(mise exec -- gh pr view "$pr" --json baseRefOid --jq .baseRefOid)"
mirror="$root/drafts/pr$pr-${head_sha:0:8}.jsonl"

if [ "$head_sha" != "$remote_head" ]; then
    echo "worktree head $head_sha differs from the PR head $remote_head; refusing to review a stale tree" >&2
    exit 2
fi

if [ -n "$(mise exec -- git -C "$worktree" status --porcelain --untracked-files=no)" ]; then
    echo "worktree has uncommitted changes to tracked files; refusing to review something other than the PR" >&2
    exit 4
fi

base_ref="refs/upwell-review/pr$pr-base"

mise exec -- git -C "$worktree" fetch -q origin "+refs/heads/$base:$base_ref"

base_tip="$(mise exec -- git -C "$worktree" rev-parse "$base_ref")"
merge_base="$(mise exec -- git -C "$worktree" merge-base "$base_tip" "$head_sha")"

mise exec -- git -C "$worktree" diff "$merge_base...$head_sha" > "$diff_file"
: > "$drafts"

{
    cat "$here/review-prompt.md"
    printf '\nThis is PR #%s, head %s. Its base branch is %s (a moving integration branch, currently at %s); review exactly `git diff %s...%s`. Those are fixed commit ids, so do not substitute branch names. Prioritise correctness, concurrency, layering and behaviour changes hidden in refactors over style nits.\n' "$pr" "$head_sha" "$base" "$base_tip" "$merge_base" "$head_sha"
    printf '\nDIFF FIRST: the complete diff is saved at `%s`. Read it first, in full, before anything else. Open other files in the working directory only when you need them to verify a finding (callers, definitions, tests); do not explore the repository broadly.\n' "$diff_file"
    cat "$here/review-drafts-prompt.md"
    printf '\nFINAL ANSWER FORMAT: reply with JSON only, matching the provided schema. `path` is repo-relative and `line` is a line number in the NEW version of the file that is part of the PR diff (prefer a changed line). Put a concrete failure scenario in `body`. Use verdict NO_FINDINGS with an empty findings list when you found nothing you could verify. `suggestion` may be an empty string. `unchecked` says what you could not check (for example, that you did not run the tests).\n'

    if [ "$provider" = "gemini" ]; then
        cat "$here/review-extra-checklist.md" "$here/review-gemini-checklist.md"
    fi

    if [ "$provider" = "copilot" ]; then
        cat "$here/review-extra-checklist.md" "$here/review-copilot-checklist.md"
    fi

    if [ -n "$focus_file" ]; then
        printf '\n# FOCUS FOR THIS REVIEW\n\nRestrict your findings to the focus below; do not report anything outside it.\n\n'
        cat "$focus_file"
        printf '\n'
    fi
} > "$prompt"

rm -f "$out"

agy_settings='{"permissions":{"allow":["mcp(blast-radius/open_project)","mcp(blast-radius/find_symbol)","mcp(blast-radius/symbol_info)","mcp(blast-radius/blast_radius)","mcp(blast-radius/diff_blast)","mcp(blast-radius/call_slice)","mcp(blast-radius/module_graph)","mcp(blast-radius/dead_code)","mcp(codegraph/codegraph_explore)"]}}'
filter=(python3 -I "$here/stream_drafts.py" "$provider" "$log" "$drafts" "$mirror" "$out")

copilot_tools=(view glob rg
    blast-radius-blast_radius blast-radius-find_symbol blast-radius-symbol_info blast-radius-call_slice
    blast-radius-module_graph blast-radius-dead_code blast-radius-diff_blast codegraph-codegraph_explore)
copilot_denied=(shell write memory url)
copilot_disabled_mcps=(MCP_DOCKER playwright railway)

set +e
case "$provider" in
    codex)
        codex exec -m "$model" -c "model_reasoning_effort=$effort" -s read-only --ephemeral \
            --output-schema "$here/review-schema.json" \
            -C "$worktree" -o "$out" - < "$prompt" 2>&1 9>&- | "${filter[@]}" 9>&-
        statuses=("${PIPESTATUS[@]}")
        ;;
    gemini)
        (
            cd "$worktree"
            agy --model "$model" --mode plan --sandbox --output-format stream-json --settings "$agy_settings" \
                --json-schema "$here/review-schema.json" --add-dir "$worktree" --add-dir "$work" \
                -p "$(cat "$prompt")" 2>&1
        ) 9>&- | "${filter[@]}" 9>&-
        statuses=("${PIPESTATUS[@]}")
        ;;
    copilot)
        copilot_args=(--model "$model" --reasoning-effort "$effort" --output-format json --no-ask-user
            --disable-builtin-mcps --disable-client-telemetry --no-auto-update
            -C "$worktree" --add-dir "$work" --available-tools "${copilot_tools[@]}")

        for denied in "${copilot_denied[@]}"; do
            copilot_args+=("--deny-tool=$denied")
        done

        for server in "${copilot_disabled_mcps[@]}"; do
            copilot_args+=(--disable-mcp-server "$server")
        done

        (
            cd "$worktree"
            copilot -p "$(cat "$prompt")" "${copilot_args[@]}" 2>&1
        ) 9>&- | "${filter[@]}" 9>&-
        statuses=("${PIPESTATUS[@]}")
        ;;
esac
set -e

draft_count="$(wc -l < "$drafts" | tr -d ' ')"

report_drafts() {
    echo "$draft_count local draft finding(s) kept in $drafts (mirror: $mirror)" >&2
    echo "inspect with: python3 -I $here/drafts.py $drafts" >&2
}

if [ "${statuses[0]}" -ne 0 ] || [ "${statuses[1]}" -ne 0 ]; then
    echo "the $provider run failed (provider exit ${statuses[0]}, filter exit ${statuses[1]}); not posting (see $log; the full answer text, if any, is in $work/response.md)" >&2
    report_drafts
    exit 10
fi

if ! python3 -I "$here/validate_review.py" "$here/review-schema.json" "$out"; then
    echo "review output is not valid structured JSON; not posting (see $log)" >&2
    report_drafts
    exit 3
fi

current_head="$(mise exec -- gh pr view "$pr" --json headRefOid --jq .headRefOid)"
current_base="$(mise exec -- gh pr view "$pr" --json baseRefOid --jq .baseRefOid)"
local_head="$(mise exec -- git -C "$worktree" rev-parse HEAD)"

if [ "$local_head" != "$head_sha" ] || [ -n "$(mise exec -- git -C "$worktree" status --porcelain --untracked-files=no)" ]; then
    echo "the local checkout changed while the review ran; not posting (see $work)" >&2
    exit 7
fi

if [ "$current_head" != "$head_sha" ]; then
    echo "the PR head moved to $current_head while the review ran; not posting a stale review (see $work)" >&2
    exit 5
fi

if [ "$current_base" != "$base_sha" ]; then
    echo "the PR base moved to $current_base while the review ran; not posting a review of a changed diff (see $work)" >&2
    exit 6
fi

echo "review $out; $draft_count local draft(s) in $drafts"
python3 -I "$here/post_review.py" "$pr" "$head_sha" "$provider" "$model" "$effort" "$out"
