"""Posts a provider review (structured JSON) to a PR as a GitHub review with inline comments.

Usage: post_review.py <pr> <head_sha> <provider> <model> <effort> <review.json>
With UPWELL_REVIEW_DRY_RUN=1 nothing is posted; a payload summary is printed instead.
Findings whose line is not part of the PR diff (right side) go into the review body instead.
"""

import json
import os
import re
import subprocess
import sys

REPO = "upwell-rs/upwell"
SEVERITY_ORDER = {"blocker": 0, "major": 1, "minor": 2, "nit": 3}


def gh(*args, stdin=None):
    command = ["mise", "exec", "--", "gh", *args]
    result = subprocess.run(command, input=stdin, capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"gh {' '.join(args[:3])} failed: {result.stderr.strip()}")
    return result.stdout


def diff_lines(pr):
    files = json.loads(gh("api", "--paginate", "--slurp", f"repos/{REPO}/pulls/{pr}/files"))
    visible = {}
    for page in files:
        for entry in page:
            lines = set()
            new = 0
            for raw in (entry.get("patch") or "").splitlines():
                hunk = re.match(r"@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@", raw)
                if hunk:
                    new = int(hunk.group(1))
                    continue
                if raw.startswith("-") or raw.startswith("\\"):
                    continue
                lines.add(new)
                new += 1
            visible[entry["filename"]] = lines
    return visible


def comment_body(finding):
    text = f"**[{finding['severity']}] {finding['title']}**\n\n{finding['body']}"
    if finding["suggestion"].strip():
        text += f"\n\n**Suggested fix:** {finding['suggestion']}"
    return text


def main():
    pr, head, provider, model, effort, path = sys.argv[1:7]
    label = {"codex": "Codex", "gemini": "Gemini", "copilot": "Copilot"}[provider]
    with open(path, encoding="utf-8") as handle:
        review = json.load(handle)

    visible = diff_lines(pr)
    findings = sorted(review["findings"], key=lambda f: SEVERITY_ORDER[f["severity"]])
    inline, loose = [], []
    for finding in findings:
        if finding["line"] in visible.get(finding["path"], set()):
            inline.append(
                {
                    "path": finding["path"],
                    "line": finding["line"],
                    "side": "RIGHT",
                    "body": comment_body(finding),
                }
            )
        else:
            loose.append(finding)

    header = f"## Independent review ({label}) of `{head[:8]}`\n\n**Verdict:** {review['verdict'].replace('_', ' ')}\n\n{review['summary']}"
    if loose:
        header += "\n\n### Findings outside the changed lines\n"
        for finding in loose:
            header += f"\n- **[{finding['severity']}]** `{finding['path']}:{finding['line']}` {finding['title']}: {finding['body']}"
            if finding["suggestion"].strip():
                header += f" Suggested fix: {finding['suggestion']}"
    if review["unchecked"].strip():
        header += f"\n\n_Could not check:_ {review['unchecked']}"
    header += (
        f"\n\n---\n_Reviewer: {label}, provider `{provider}`, model `{model}`, reasoning effort {effort}, read-only sandbox. "
        "Replaces CodeRabbit while the organisation is on the Free tier. Each inline comment is a resolvable thread; "
        "the author answers and resolves it after pushing. This is not a bot approval._"
    )

    payload = {"commit_id": head, "event": "COMMENT", "body": header, "comments": inline}
    if os.environ.get("UPWELL_REVIEW_DRY_RUN") == "1":
        print(f"DRY RUN, not posted: verdict {review['verdict']}, {len(inline)} inline, {len(loose)} in the body")
        print(header)
        return
    out = gh("api", "-X", "POST", f"repos/{REPO}/pulls/{pr}/reviews", "--input", "-", stdin=json.dumps(payload))
    url = json.loads(out).get("html_url", "")
    print(f"posted review of #{pr} at {head[:8]}: {len(inline)} inline, {len(loose)} in the body, {url}")


main()
