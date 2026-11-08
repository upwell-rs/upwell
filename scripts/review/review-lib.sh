# Shared functions of review-pr.sh, sourced by it. They read the globals `here` (this directory),
# `worktree` (the PR checkout) and `work` (the run directory).

copilot_tools=(view glob rg
    blast-radius-blast_radius blast-radius-find_symbol blast-radius-symbol_info blast-radius-call_slice
    blast-radius-module_graph blast-radius-dead_code blast-radius-diff_blast codegraph-codegraph_explore)
copilot_denied=(shell write memory url)
copilot_disabled_mcps=(MCP_DOCKER playwright railway)
agy_settings='{"permissions":{"allow":["mcp(blast-radius/open_project)","mcp(blast-radius/find_symbol)","mcp(blast-radius/symbol_info)","mcp(blast-radius/blast_radius)","mcp(blast-radius/diff_blast)","mcp(blast-radius/call_slice)","mcp(blast-radius/module_graph)","mcp(blast-radius/dead_code)","mcp(codegraph/codegraph_explore)"]}}'

check_provider() {
    case "$1" in
        codex | gemini | copilot) ;;
        *)
            echo "unknown provider '$1'; expected codex, gemini or copilot" >&2
            exit 8
            ;;
    esac
}

# Prints the effort to record for a run: gemini models carry their effort in the model name.
effort_for() {
    if [ "$1" = "gemini" ]; then
        echo "set by the model name"
        return
    fi

    echo "$2"
}

# Takes an exclusive flock(2) on the per-model lock file $1 through fd 9, which this shell keeps
# open until it exits or calls release_lock, and records this pid next to it for the refusal
# message. The kernel releases the lock however the process ends, so there is no stale lock to
# take over and no lock to delete. The long-running provider commands get fd 9 closed (9>&-) so
# an orphan of a killed run cannot keep holding it.
hold_lock() {
    exec 9>> "$1"

    python3 -I -c 'import fcntl; fcntl.flock(9, fcntl.LOCK_EX | fcntl.LOCK_NB)' 2> /dev/null || return 1
    echo "$$" > "$1.pid"
}

release_lock() {
    exec 9>&-
}

# Takes the lock of provider $1 and model $2 or exits 9; $3 is an extra hint for the refusal.
lock_model() {
    local lock="$root/locks/$1--$2.lock"
    local holder=""

    if hold_lock "$lock"; then
        return
    fi

    holder="$(cat "$lock.pid" 2> /dev/null || true)"

    echo "another review with $1/$2 is already running (pid ${holder:-unknown}, lock $lock); run one instance at a time${3:+; $3}" >&2
    exit 9
}

copilot_arguments() {
    local model="$1" effort="$2"
    local denied="" server=""

    copilot_args=(--model "$model" --reasoning-effort "$effort" --output-format json --no-ask-user
        --disable-builtin-mcps --disable-client-telemetry --no-auto-update
        -C "$worktree" --add-dir "$work" --available-tools "${copilot_tools[@]}")

    for denied in "${copilot_denied[@]}"; do
        copilot_args+=("--deny-tool=$denied")
    done

    for server in "${copilot_disabled_mcps[@]}"; do
        copilot_args+=(--disable-mcp-server "$server")
    done
}

# Runs one fresh, read-only provider session (no resume, no shared state with another stage) on
# a prompt file and filters its output through stream_drafts.py.
# Arguments: provider model effort schema prompt answer log [drafts mirror stamp]; the drafts
# arguments are given only in discovery. Fails when the provider or the filter failed and leaves
# both exit codes in run_statuses.
run_provider() {
    local provider="$1" model="$2" effort="$3" schema="$4" prompt="$5" answer="$6" log="$7"
    local filter=(python3 -I "$here/stream_drafts.py" "$provider" "$log" "$answer" "${@:8}")

    rm -f "$answer"
    set +e

    case "$provider" in
        codex)
            codex exec -m "$model" -c "model_reasoning_effort=$effort" -s read-only --ephemeral \
                --output-schema "$schema" -C "$worktree" -o "$answer" - < "$prompt" 2>&1 9>&- | "${filter[@]}" 9>&-
            run_statuses=("${PIPESTATUS[@]}")
            ;;
        gemini)
            (
                cd "$worktree"
                agy --model "$model" --mode plan --sandbox --output-format stream-json --settings "$agy_settings" \
                    --json-schema "$schema" --add-dir "$worktree" --add-dir "$work" -p "$(cat "$prompt")" 2>&1
            ) 9>&- | "${filter[@]}" 9>&-
            run_statuses=("${PIPESTATUS[@]}")
            ;;
        copilot)
            copilot_arguments "$model" "$effort"
            (
                cd "$worktree"
                copilot -p "$(cat "$prompt")" "${copilot_args[@]}" 2>&1
            ) 9>&- | "${filter[@]}" 9>&-
            run_statuses=("${PIPESTATUS[@]}")
            ;;
    esac

    set -e

    [ "${run_statuses[0]}" -eq 0 ] && [ "${run_statuses[1]}" -eq 0 ]
}

# Appends the provider's tool notes (and, for discovery, the extra checklist) to a prompt.
provider_notes() {
    local provider="$1" stage="$2"

    if [ "$provider" = "codex" ]; then
        return
    fi

    if [ "$stage" = "discovery" ]; then
        cat "$here/review-extra-checklist.md"
    fi

    cat "$here/review-$provider-checklist.md"
}

answer_schema() {
    printf '\nThe final answer must validate against this JSON schema:\n\n```json\n'
    cat "$1"
    printf '```\n'
}

# Exits 7, 5 or 6 when the local checkout, the PR head or the PR base changed since the run began.
check_fresh() {
    local current_head="" current_base="" local_head=""

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
}
