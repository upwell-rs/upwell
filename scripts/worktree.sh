#!/usr/bin/env bash
#
# Creates and removes isolated worktrees for parallel work, usually on a large external drive.
#
#   worktree.sh add <slice> <branch> <base>
#   worktree.sh remove <slice> [--force]
#   worktree.sh seed <slice>
#   worktree.sh list
#
# Worktrees live in $UPWELL_WORKTREE_ROOT/worktrees/<slice> (default: ../upwell-worktrees beside
# the main checkout), each with its own `target/`. Never share a target or build directory between
# worktrees: Cargo gives one package at two paths the same identity, so a worktree can run another
# worktree's code and report PASS.
#
# To avoid full rebuilds, `add` clones $UPWELL_WORKTREE_ROOT/seed copy-on-write when it exists, then
# touches the tracked files so the workspace crates rebuild while dependencies are reused. That
# touch is what makes the clone safe. `seed` makes a slice's `target/` the new seed. A dependency's
# build-script output can still name the path it was built at; if a build fails on a path from
# another worktree or target directory, `cargo clean -p <crate>` in the new worktree rebuilds it.
#
# Worktrees are locked, because `git worktree prune` would otherwise forget them while the drive
# is unplugged. The gitignored `.env` is copied so `mise exec -- git` commits with the right identity.

set -euo pipefail

common_dir="$(git rev-parse --path-format=absolute --git-common-dir)"
repo="$(dirname "$common_dir")"
root="${UPWELL_WORKTREE_ROOT:-$(dirname "$repo")/upwell-worktrees}"
worktrees="$root/worktrees"
seed="$root/seed"

fail() {
    echo "worktree: $1" >&2
    exit 1
}

require_root() {
    mkdir -p "$worktrees" 2>/dev/null || fail "cannot create $worktrees; is the drive connected?"
}

# Holds off other `add` and `seed` runs, so a refresh never replaces the seed during a clone.
lock_seed() {
    local waited=0 error

    until error="$(LC_ALL=C mkdir "$seed.lock" 2>&1)"; do
        [[ "$error" == *"File exists"* ]] || fail "cannot create $seed.lock: $error"
        (( waited < 300 )) || fail "$seed.lock held for 5 minutes; if nothing is using it, remove it"

        sleep 1
        waited=$(( waited + 1 ))
    done

    trap unlock_seed EXIT
}

unlock_seed() {
    rm -rf "$seed.lock"
    trap - EXIT
}

clone_tree() {
    if [[ "$(uname -s)" == "Darwin" ]]; then
        cp -cR "$1" "$2"
    else
        cp -R --reflink=auto "$1" "$2"
    fi
}

# Undoes an `add` that failed after registering its worktree, so the slice can be retried.
rollback_add() {
    git -C "$repo" worktree unlock "$adding_path" 2> /dev/null || true
    git -C "$repo" worktree remove --force "$adding_path" 2> /dev/null || true

    if [[ -e "$adding_path" ]]; then
        echo "worktree: could not remove $adding_path; delete it, then run: git worktree prune && git branch -D $adding_branch" >&2
    else
        git -C "$repo" branch --quiet -D "$adding_branch" 2> /dev/null || true
    fi

    unlock_seed
}

# Puts back a seed that an interrupted or failed refresh moved aside.
recover_seed() {
    if [[ -d "$seed.previous" && ! -d "$seed" ]]; then
        mv "$seed.previous" "$seed"
    fi
}

add() {
    local slice="$1" branch="$2" base="$3"
    local path="$worktrees/$slice"
    local start="$base"

    require_root

    git -C "$repo" fetch --quiet origin || echo "worktree: fetch failed; using local refs" >&2

    if git -C "$repo" rev-parse --verify --quiet "origin/$base" > /dev/null; then
        start="origin/$base"
    fi

    lock_seed
    recover_seed

    [[ -e "$path" ]] && fail "$path already exists"
    git -C "$repo" show-ref --verify --quiet "refs/heads/$branch" && fail "branch $branch already exists"

    adding_path="$path"
    adding_branch="$branch"
    trap rollback_add EXIT
    git -C "$repo" worktree add --quiet --no-track -b "$branch" "$path" "$start"
    git -C "$repo" worktree lock --reason "parallel worktree on $root" "$path"

    if [[ -f "$repo/.env" ]]; then
        cp "$repo/.env" "$path/.env"
    fi

    (cd "$path" && mise trust --quiet 2> /dev/null) || true

    if [[ -d "$seed" ]]; then
        clone_tree "$seed" "$path/target"
        git -C "$path" ls-files -z | (cd "$path" && xargs -0 touch)
        echo "worktree: seeded target from $seed"
    fi

    unlock_seed

    echo "worktree: $path on $branch from $start"
}

# The ignored files of the worktree at `$1` that `add` didn't create: anything but build output
# (`target/`, `benchmarks/target/`) and an unedited copy of `.env`.
local_ignored_files() {
    local entry

    git -C "$1" status --porcelain --ignored | sed -n 's/^!! //p' | while read -r entry; do
        case "$entry" in
            target/ | benchmarks/target/) ;;
            .env) cmp -s "$1/.env" "$repo/.env" || echo "$entry" ;;
            *) echo "$entry" ;;
        esac
    done
}

remove() {
    local slice="$1" force="${2:-}"
    local path="$worktrees/$slice"
    local unpushed ignored

    [[ -d "$path" ]] || fail "$path does not exist; is the drive connected?"

    lock_seed

    if [[ "$force" != "--force" ]]; then
        unpushed="$(git -C "$path" log --oneline HEAD --not --remotes | wc -l | tr -d ' ')"

        [[ "$unpushed" == "0" ]] || fail "$slice has $unpushed unpushed commits; push them or pass --force"

        ignored="$(local_ignored_files "$path")"

        [[ -z "$ignored" ]] || fail "$slice holds ignored local files, pass --force to delete them: $ignored"
    fi

    git -C "$repo" worktree unlock "$path"

    if ! git -C "$repo" worktree remove ${force:+"$force"} "$path"; then
        git -C "$repo" worktree lock --reason "parallel worktree on $root" "$path"
        fail "could not remove $slice; commit its changes or pass --force"
    fi

    git -C "$repo" worktree prune
    unlock_seed
    echo "worktree: removed $path"
}

refresh_seed() {
    local target="$worktrees/$1/target"
    local next="$seed.next"
    local previous="$seed.previous"

    [[ -d "$target" ]] || fail "$target does not exist"

    lock_seed
    recover_seed
    rm -rf "$next" "$previous"
    clone_tree "$target" "$next"

    if [[ -d "$seed" ]]; then
        mv "$seed" "$previous"
    fi

    if ! mv "$next" "$seed"; then
        recover_seed
        fail "could not publish the new seed; kept the previous one"
    fi

    rm -rf "$previous"
    unlock_seed
    echo "worktree: seed refreshed from $1"
}

list() {
    git -C "$repo" worktree list

    if [[ -d "$seed" ]]; then
        echo "seed: $seed"
    fi
}

case "${1:-}" in
    add) [[ $# == 4 ]] || fail "usage: add <slice> <branch> <base>"; add "$2" "$3" "$4" ;;
    remove) [[ $# -ge 2 ]] || fail "usage: remove <slice> [--force]"; remove "$2" "${3:-}" ;;
    seed) [[ $# == 2 ]] || fail "usage: seed <slice>"; refresh_seed "$2" ;;
    list) list ;;
    *) fail "usage: worktree.sh add|remove|seed|list" ;;
esac
