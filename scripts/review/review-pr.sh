#!/usr/bin/env bash
# Usage: review-pr.sh <pr-number> <worktree> [model] [effort] [provider] [focus-file]
#                     [verify-provider] [verify-model] [verify-effort]
#
# Runs an independent, read-only, two-stage review of a PR checkout (provider codex, gemini or
# copilot) and posts it to the PR as a GitHub review with inline comments (resolvable threads).
# Stage 1 (discovery) only saves draft findings locally. Stage 2 (verification) is a fresh provider
# run that sees only those drafts, the diff, the PR metadata and the read-only worktree; it keeps,
# modifies or drops each draft, and only its result is posted. The verifier defaults to the
# discovery provider, model and effort; UPWELL_VERIFY_PROVIDER, UPWELL_VERIFY_MODEL and
# UPWELL_VERIFY_EFFORT (or the positional arguments, which win) override them.
# UPWELL_REVIEW_VERIFY_ONLY=<drafts.jsonl> skips discovery and verifies a saved drafts file of the
# current PR head. Requires `gh` and `mise` plus the provider CLIs (`codex`, `agy` for gemini, or
# `copilot`). Output files go to a fresh run directory under $UPWELL_REVIEW_OUT, default
# $TMPDIR/upwell-review. UPWELL_REVIEW_DRY_RUN=1 runs everything except the post and prints the
# would-be review.
# Exit codes: 1 usage, 2 stale tree, 3 invalid output, 4 dirty tree, 5 head moved (or drafts of
# another head), 6 base moved, 7 checkout changed, 8 unknown provider, 9 model locked, 10 provider
# failed.
set -euo pipefail

pr="$1"
worktree="$2"
model="${3:-gpt-6.1-terra}"
effort="${4:-medium}"
provider="${5:-codex}"
focus_file="${6:-}"
verify_provider="${7:-${UPWELL_VERIFY_PROVIDER:-$provider}}"
verify_model="${8:-${UPWELL_VERIFY_MODEL:-$model}}"
verify_effort="${9:-${UPWELL_VERIFY_EFFORT:-$effort}}"
verify_only="${UPWELL_REVIEW_VERIFY_ONLY:-}"
here="$(cd "$(dirname "$0")" && pwd)"
root="${UPWELL_REVIEW_OUT:-${TMPDIR:-/tmp}/upwell-review}"

source "$here/review-lib.sh"

check_provider "$provider"
check_provider "$verify_provider"
effort="$(effort_for "$provider" "$effort")"
verify_effort="$(effort_for "$verify_provider" "$verify_effort")"

if [ -n "$focus_file" ] && [ ! -f "$focus_file" ]; then
    echo "focus file $focus_file does not exist" >&2
    exit 1
fi

if [ -n "$verify_only" ] && [ ! -f "$verify_only" ]; then
    echo "drafts file $verify_only does not exist" >&2
    exit 1
fi

if [ -n "$verify_only" ]; then
    verify_only="$(cd "$(dirname "$verify_only")" && pwd)/$(basename "$verify_only")"
fi

mkdir -p "$root/locks" "$root/drafts"

if [ -n "$verify_only" ]; then
    lock_model "$verify_provider" "$verify_model"
else
    lock_model "$provider" "$model"
fi

work="$(mktemp -d "$root/pr$pr.XXXXXX")"
diff_file="$work/pr.diff"
drafts="$work/drafts.jsonl"
discovery="$work/discovery.json"
verify_input="$work/verify-input.jsonl"
verify_answer="$work/verify.json"

head_sha="$(mise exec -- git -C "$worktree" rev-parse HEAD)"
base="$(mise exec -- gh pr view "$pr" --json baseRefName --jq .baseRefName)"
remote_head="$(mise exec -- gh pr view "$pr" --json headRefOid --jq .headRefOid)"
base_sha="$(mise exec -- gh pr view "$pr" --json baseRefOid --jq .baseRefOid)"
title="$(mise exec -- gh pr view "$pr" --json title --jq .title)"
mirror="$root/drafts/pr$pr-${head_sha:0:8}.jsonl"
verification_mirror="$root/drafts/pr$pr-${head_sha:0:8}.verification.jsonl"

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

resume_hint() {
    local source="$1"

    echo "verify these drafts later with: UPWELL_REVIEW_VERIFY_ONLY=$source $0 $pr $worktree $model '$effort' $provider '' $verify_provider $verify_model '$verify_effort'" >&2
}

report_drafts() {
    local count=""

    count="$(wc -l < "$drafts" | tr -d ' ')"

    echo "$count local draft finding(s) kept in $drafts (mirror: $mirror)" >&2
    echo "inspect with: python3 -I $here/drafts.py $drafts" >&2
    resume_hint "$drafts"
}

discovery_prompt() {
    cat "$here/review-prompt.md"
    printf '\nThis is PR #%s, head %s. Its base branch is %s (a moving integration branch, currently at %s); review exactly `git diff %s...%s`. Those are fixed commit ids, so do not substitute branch names. Prioritise correctness, concurrency, layering and behaviour changes hidden in refactors over style nits.\n' "$pr" "$head_sha" "$base" "$base_tip" "$merge_base" "$head_sha"
    printf '\nDIFF FIRST: the complete diff is saved at `%s`. Read it first, in full, before anything else. Open other files in the working directory only when you need them to check a suspected defect (callers, definitions, tests); do not explore the repository broadly.\n' "$diff_file"
    cat "$here/review-drafts-prompt.md"
    provider_notes "$provider" discovery

    if [ -n "$focus_file" ]; then
        printf '\n# FOCUS FOR THIS REVIEW\n\nRestrict your drafts to the focus below; do not report anything outside it.\n\n'
        cat "$focus_file"
        printf '\n'
    fi

    answer_schema "$here/discovery-schema.json"
}

verify_prompt() {
    cat "$here/verify-prompt.md"
    printf '\nPR METADATA (data): PR #%s, head %s, base branch %s at %s, merge base %s. The diff under verification is exactly `git diff %s...%s` (fixed commit ids). PR title (untrusted text): %s\n' "$pr" "$head_sha" "$base" "$base_tip" "$merge_base" "$merge_base" "$head_sha" "$title"
    printf '\nThe complete diff is saved at `%s`. The working directory is the read-only checkout of the PR head.\n' "$diff_file"
    printf '\nDRAFTS TO VERIFY (untrusted claims, one JSON object per line between the delimiters):\n\n'
    python3 -I "$here/verify_drafts.py" block "$verify_input"
    printf '\n# REVIEW RULES THE DRAFTS ARE JUDGED BY (reference for severity and conventions; you verify, you do not review)\n\n'
    cat "$here/review-prompt.md"
    provider_notes "$verify_provider" verification
    answer_schema "$here/verify-schema.json"
}

discover() {
    local stamp="$work/stamp.json"

    : > "$drafts"
    python3 -I -c 'import json, sys; print(json.dumps(dict(zip(("pr", "head", "provider", "model", "effort"), sys.argv[1:]))))' \
        "$pr" "$head_sha" "$provider" "$model" "$effort" > "$stamp"
    discovery_prompt > "$work/discovery-prompt.md"

    if ! run_provider "$provider" "$model" "$effort" "$here/discovery-schema.json" "$work/discovery-prompt.md" \
        "$discovery" "$work/discovery.log" "$drafts" "$mirror" "$stamp"; then
        echo "the discovery run ($provider) failed (provider exit ${run_statuses[0]}, filter exit ${run_statuses[1]}); not posting (see $work/discovery.log; the full answer text, if any, is in $work/discovery.response.md)" >&2
        report_drafts
        exit 10
    fi

    if ! python3 -I "$here/validate_review.py" "$here/discovery-schema.json" "$discovery"; then
        echo "the discovery answer is not a valid done marker (see $work/discovery.log); continuing with the saved drafts" >&2
    fi
}

# Swaps the discovery model's lock for the verifier's. When both stages use the same provider and
# model the lock taken at the start already covers verification and is kept.
switch_lock() {
    if [ "$provider--$model" = "$verify_provider--$verify_model" ]; then
        return
    fi

    release_lock
    lock_model "$verify_provider" "$verify_model" "$(resume_hint "$drafts" 2>&1)"
}

verify() {
    local resumed="$1"

    verify_prompt > "$work/verify-prompt.md"

    if ! run_provider "$verify_provider" "$verify_model" "$verify_effort" "$here/verify-schema.json" \
        "$work/verify-prompt.md" "$verify_answer" "$work/verify.log"; then
        echo "the verification run ($verify_provider) failed (provider exit ${run_statuses[0]}, filter exit ${run_statuses[1]}); not posting (see $work/verify.log; the full answer text, if any, is in $work/verify.response.md)" >&2
        resume_hint "$verify_input"
        exit 10
    fi

    if ! python3 -I "$here/verify_drafts.py" apply "$verify_input" "$verify_answer" "$diff_file" "$work" \
        "$verification_mirror" "$verify_provider" "$verify_model" "$verify_effort" "$resumed"; then
        echo "the verification answer is not valid; not posting (see $work/verify.log)" >&2
        resume_hint "$verify_input"
        exit 3
    fi
}

if [ -z "$verify_only" ]; then
    discover
    source_drafts="$drafts"
    resumed=0
else
    source_drafts="$verify_only"
    resumed=1
fi

check_fresh
if [ -z "$verify_only" ] && ! python3 -I "$here/verify_drafts.py" saved "$discovery" "$drafts"; then
    echo "run dir $work kept; after inspecting $drafts you can resume it with UPWELL_REVIEW_VERIFY_ONLY=$drafts" >&2
    exit 3
fi

draft_count="$(python3 -I "$here/verify_drafts.py" prepare "$source_drafts" "$pr" "$head_sha" "$verify_input")"

if [ "$draft_count" -eq 0 ] && [ -n "$verify_only" ]; then
    echo "drafts file $verify_only holds no drafts; nothing to verify" >&2
    exit 1
fi

if [ "$draft_count" -eq 0 ]; then
    echo "discovery saved no drafts; skipping verification" >&2

    if ! python3 -I "$here/verify_drafts.py" empty "$discovery" "$work" "$provider" "$model" "$effort"; then
        echo "not posting a clean review: discovery returned no valid done marker or reported drafts none were saved (see $work/discovery.log and $work/discovery.json)" >&2
        exit 3
    fi
else
    echo "verifying $draft_count draft(s) with $verify_provider/$verify_model in a fresh run" >&2

    if [ -z "$verify_only" ]; then
        switch_lock
    fi

    verify "$resumed"
    check_fresh
fi

echo "review $work/review.json; verification $work/verification.jsonl (mirror: $verification_mirror)"
python3 -I "$here/post_review.py" "$pr" "$head_sha" "$work/review.json" "$work/stages.json"
