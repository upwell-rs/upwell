"""Pretty-prints local finding drafts for manual verification.

Usage: drafts.py <drafts.jsonl>
Drafts are unverified discovery output; only the verification stage's decisions are posted.
"""

import json
import sys

ORDER = {"blocker": 0, "major": 1, "minor": 2, "nit": 3}


def sort_key(draft):
    """Orders by severity, path and line; tolerates hand-edited records with odd field types."""
    line = draft.get("line")

    return ORDER.get(draft.get("severity"), 9), str(draft.get("path", "")), line if isinstance(line, int) else 0


def main():
    drafts = []

    with open(sys.argv[1], encoding="utf-8") as handle:
        for raw in handle:
            if raw.strip():
                drafts.append(json.loads(raw))

    drafts.sort(key=sort_key)
    print(f"{len(drafts)} draft(s), unverified")

    for number, draft in enumerate(drafts, 1):
        print(f"\n[{number}] {draft.get('path')}:{draft.get('line')}  {draft.get('severity')}  {draft.get('title')}")
        print(f"    {draft.get('body', '')}")

        if draft.get("suggestion"):
            print(f"    fix: {draft['suggestion']}")


main()
