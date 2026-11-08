"""Posts a verified review (structured JSON) to a PR as a GitHub review with inline comments.

Usage: post_review.py <pr> <head_sha> <review.json> <stages.json>
<stages.json> names the discovery and verification runs (provider, model, effort, counts) for the
footer. With UPWELL_REVIEW_DRY_RUN=1 nothing is posted; the would-be review body and inline
comments are printed and the payload is saved as payload.json next to <review.json>.
Findings whose line is not part of the PR diff (right side) go into the review body instead.
"""

import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from diffmap import patch_lines

REPO = "upwell-rs/upwell"
SEVERITY_ORDER = {"blocker": 0, "major": 1, "minor": 2, "nit": 3}
LABELS = {"codex": "Codex", "gemini": "Gemini", "copilot": "Copilot"}


def gh(*args, stdin=None):
    command = ["mise", "exec", "--", "gh", *args]
    result = subprocess.run(command, input=stdin, capture_output=True, text=True)

    if result.returncode != 0:
        sys.exit(f"gh {' '.join(args[:3])} failed: {result.stderr.strip()}")

    return result.stdout


def diff_lines(pr):
    files = json.loads(gh("api", "--paginate", "--slurp", f"repos/{REPO}/pulls/{pr}/files"))

    return {entry["filename"]: patch_lines(entry.get("patch") or "") for page in files for entry in page}


def comment_body(finding):
    text = f"**[{finding['severity']}] {finding['title']}**\n\n{finding['body']}"

    if finding["suggestion"].strip():
        text += f"\n\n**Suggested fix:** {finding['suggestion']}"

    return text


def run_label(run):
    return f"{LABELS.get(run['provider'], run['provider'])} `{run['model']}`, effort {run['effort']}"


def footer(stages):
    """Names both stages' provider, model and effort with the draft and decision counts."""
    discovery = stages["discovery"]
    verification = stages["verification"]
    found = "; ".join(run_label(origin) for origin in discovery["origins"])
    resumed = ", verified from a saved drafts file" if discovery["resumed"] else ""
    text = f"\n\n---\n_Discovery: {found}: {discovery['drafts']} draft(s){resumed}. "

    if verification is None:
        text += "Verification: skipped (no drafts). "
    else:
        text += (
            f"Verification (fresh run that saw only the drafts, the diff and the code): {run_label(verification)}: "
            f"{verification['keep']} kept, {verification['modify']} modified, {verification['drop']} dropped. "
        )

    return text + (
        "Read-only sandbox. Replaces CodeRabbit while the organisation is on the Free tier. Each inline comment is a "
        "resolvable thread; the author answers and resolves it after pushing. This is not a bot approval._"
    )


def title_label(stages):
    names = [LABELS.get(origin["provider"], origin["provider"]) for origin in stages["discovery"]["origins"]]
    label = ", ".join(dict.fromkeys(names))

    if stages["verification"] is not None:
        label += f", verified by {LABELS.get(stages['verification']['provider'], stages['verification']['provider'])}"

    return label


def split_findings(findings, visible):
    inline = []
    loose = []

    for finding in sorted(findings, key=lambda f: SEVERITY_ORDER[f["severity"]]):
        if finding["line"] in visible.get(finding["path"], set()):
            inline.append({"path": finding["path"], "line": finding["line"], "side": "RIGHT", "body": comment_body(finding)})
        else:
            loose.append(finding)

    return inline, loose


def body(review, head, stages, loose):
    text = f"## Independent review ({title_label(stages)}) of `{head[:8]}`\n\n**Verdict:** {review['verdict'].replace('_', ' ')}\n\n{review['summary']}"

    if loose:
        text += "\n\n### Findings outside the changed lines\n"

    for finding in loose:
        text += f"\n- **[{finding['severity']}]** `{finding['path']}:{finding['line']}` {finding['title']}: {finding['body']}"

        if finding["suggestion"].strip():
            text += f" Suggested fix: {finding['suggestion']}"

    if review["unchecked"].strip():
        text += f"\n\n_Could not check:_ {review['unchecked']}"

    return text + footer(stages)


def load(path):
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


def dry_run(review, payload, inline, loose, review_path):
    with open(os.path.join(os.path.dirname(review_path), "payload.json"), "w", encoding="utf-8") as handle:
        json.dump(payload, handle, ensure_ascii=False, indent=2)

    print(f"DRY RUN, not posted: verdict {review['verdict']}, {len(inline)} inline, {len(loose)} in the body")
    print(payload["body"])

    for comment in inline:
        print(f"\n--- inline {comment['path']}:{comment['line']}\n{comment['body']}")


def main():
    pr, head, review_path, stages_path = sys.argv[1:5]
    review = load(review_path)
    stages = load(stages_path)
    inline, loose = split_findings(review["findings"], diff_lines(pr))
    payload = {"commit_id": head, "event": "COMMENT", "body": body(review, head, stages, loose), "comments": inline}

    if os.environ.get("UPWELL_REVIEW_DRY_RUN") == "1":
        dry_run(review, payload, inline, loose, review_path)
        return

    out = gh("api", "-X", "POST", f"repos/{REPO}/pulls/{pr}/reviews", "--input", "-", stdin=json.dumps(payload))
    url = json.loads(out).get("html_url", "")

    print(f"posted review of #{pr} at {head[:8]}: {len(inline)} inline, {len(loose)} in the body, {url}")


if __name__ == "__main__":
    main()
