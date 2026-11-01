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
# touch is what makes the clone safe. `seed` makes a slice's `target/` the new seed.
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

clone_tree() {
    if [[ "$(uname -s)" == "Darwin" ]]; then
        cp -cR "$1" "$2"
    else
        cp -R --reflink=auto "$1" "$2"
    fi
}

add() {
    local slice="$1" branch="$2" base="$3"
    local path="$worktrees/$slice"
    local start="$base"

    require_root
    [[ -e "$path" ]] && fail "$path already exists"

    git -C "$repo" fetch --quiet origin

    if git -C "$repo" rev-parse --verify --quiet "origin/$base" > /dev/null; then
        start="origin/$base"
    fi

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

    echo "worktree: $path on $branch from $start"
}

remove() {
    local slice="$1" force="${2:-}"
    local path="$worktrees/$slice"
    local unpushed

    [[ -d "$path" ]] || fail "$path does not exist; is the drive connected?"

    if [[ "$force" != "--force" ]]; then
        unpushed="$(git -C "$path" log --oneline HEAD --not --remotes | wc -l | tr -d ' ')"

        [[ "$unpushed" == "0" ]] || fail "$slice has $unpushed unpushed commits; push them or pass --force"
    fi

    git -C "$repo" worktree unlock "$path"

    if [[ "$force" == "--force" ]]; then
        git -C "$repo" worktree remove --force "$path"
    elif ! git -C "$repo" worktree remove "$path"; then
        git -C "$repo" worktree lock --reason "parallel worktree on $root" "$path"
        fail "$slice has uncommitted changes; commit them or pass --force"
    fi

    git -C "$repo" worktree prune
    echo "worktree: removed $path"
}

refresh_seed() {
    local target="$worktrees/$1/target"
    local next="$seed.next"

    [[ -d "$target" ]] || fail "$target does not exist"

    rm -rf "$next"
    clone_tree "$target" "$next"
    rm -rf "$seed"
    mv "$next" "$seed"
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
