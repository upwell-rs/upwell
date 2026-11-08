"""Maps unified diffs to the new-file (right side) line numbers a review comment may target.

A line is commentable when it is an added or a context line of a hunk, the rule GitHub applies to
review comments on the RIGHT side.
"""

import re

HUNK = re.compile(r"@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@")
OCTAL = re.compile(r"[0-7]{3}")
ESCAPES = {"a": 7, "b": 8, "t": 9, "n": 10, "v": 11, "f": 12, "r": 13, '"': 34, "\\": 92}


def patch_lines(patch):
    """Returns the right-side line numbers of one file's hunks (a GitHub `patch` field)."""
    lines = set()
    new = 0

    for raw in patch.removesuffix("\n").split("\n"):
        hunk = HUNK.match(raw)

        if hunk:
            new = int(hunk.group(1))
            continue

        if new == 0 or raw.startswith("-") or raw.startswith("\\"):
            continue

        lines.add(new)
        new += 1

    return lines


def unquote(quoted):
    """Decodes the body of a path git C-quoted (octal byte escapes and `\\t`, `\\"`, `\\\\` and the like)."""
    decoded = bytearray()
    index = 0

    while index < len(quoted):
        char = quoted[index]
        escape = quoted[index + 1:index + 2]

        if char == "\\" and OCTAL.fullmatch(quoted[index + 1:index + 4]):
            decoded.append(int(quoted[index + 1:index + 4], 8))
            index += 4
            continue

        if char == "\\" and escape in ESCAPES:
            decoded.append(ESCAPES[escape])
            index += 2
            continue

        decoded.extend(char.encode("utf-8"))
        index += 1

    return decoded.decode("utf-8", errors="replace")


def new_path(header):
    """Returns the repo-relative path of a `+++ b/<path>` header, or None for a deleted file."""
    path = header[4:].rstrip("\r\n\t")

    if len(path) >= 2 and path.startswith('"') and path.endswith('"'):
        path = unquote(path[1:-1])

    if path == "/dev/null":
        return None

    return path[2:] if path.startswith("b/") else path


def diff_lines(diff):
    """Returns {path: right-side line numbers} for a full `git diff` output."""
    files = {}
    path = None
    patch = []

    for raw in diff.removesuffix("\n").split("\n"):
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
