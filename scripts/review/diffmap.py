"""Maps unified diffs to the new-file (right side) line numbers a review comment may target.

A line is commentable when it is an added or a context line of a hunk, the rule GitHub applies to
review comments on the RIGHT side.
"""

import re

HUNK = re.compile(r"@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@")


def patch_lines(patch):
    """Returns the right-side line numbers of one file's hunks (a GitHub `patch` field)."""
    lines = set()
    new = 0

    for raw in patch.splitlines():
        hunk = HUNK.match(raw)

        if hunk:
            new = int(hunk.group(1))
            continue

        if new == 0 or raw.startswith("-") or raw.startswith("\\"):
            continue

        lines.add(new)
        new += 1

    return lines


def new_path(header):
    """Returns the repo-relative path of a `+++ b/<path>` header, or None for a deleted file."""
    path = header[4:].strip()

    if path.startswith('"') and path.endswith('"'):
        path = path[1:-1]

    if path == "/dev/null":
        return None

    return path[2:] if path.startswith("b/") else path


def diff_lines(diff):
    """Returns {path: right-side line numbers} for a full `git diff` output."""
    files = {}
    path = None
    patch = []

    for raw in diff.splitlines():
        if raw.startswith("diff --git "):
            if path is not None:
                files[path] = patch_lines("\n".join(patch))

            path = None
            patch = []
            continue

        if not patch and raw.startswith("+++ "):
            path = new_path(raw)
            continue

        if raw.startswith("@@") or patch:
            patch.append(raw)

    if path is not None:
        files[path] = patch_lines("\n".join(patch))

    return files
