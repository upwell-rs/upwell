"""Prepares discovery drafts for the verification stage and turns its decisions into the review to post.

Usage:
  verify_drafts.py prepare <drafts.jsonl> <pr> <head> <input.jsonl>
      Checks that every draft belongs to PR <pr> at <head> (exit 5 otherwise), drops duplicates,
      assigns ids D1..Dn and writes <input.jsonl>. Prints the number of drafts.
  verify_drafts.py block <input.jsonl>
      Prints the drafts as id-tagged JSON lines between delimiters, for the verification prompt.
  verify_drafts.py apply <input.jsonl> <verify.json> <pr.diff> <run dir> <mirror.jsonl> <provider> <model> <effort> <resumed 0|1>
      Validates the verifier's answer (exit 3 on an unknown, duplicate or missing id, a schema
      violation or a kept line outside the diff) and writes review.json, verification.jsonl and stages.json to
      <run dir>; verification.jsonl is also appended to <mirror.jsonl>.
  verify_drafts.py saved <discovery.json> <drafts.jsonl>
      Exit 3 when a valid done marker reports more drafts than <drafts.jsonl> holds.
  verify_drafts.py empty <discovery.json> <run dir> <provider> <model> <effort>
      Writes the NO_FINDINGS review.json and stages.json for a discovery without drafts (exit 3
      when the discovery answer is not a valid done marker or reports drafts none were saved).
"""

import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from diffmap import diff_lines
from validate_review import load, violation

HERE = os.path.dirname(os.path.abspath(__file__))
FINDING = ("severity", "path", "line", "title", "body", "suggestion")
ORIGIN = ("provider", "model", "effort")
BEGIN = "<<<UPWELL_DRAFTS_BEGIN>>>"
END = "<<<UPWELL_DRAFTS_END>>>"
MIRROR_NAME = re.compile(r"pr(\d+)-([0-9a-f]{8})\.jsonl$")


class Invalid(Exception):
    """A verifier answer that must not be posted; the message names the violation."""


def read_jsonl(path):
    records = []

    with open(path, encoding="utf-8") as handle:
        for raw in handle:
            if raw.strip():
                records.append(json.loads(raw))

    return records


def write_jsonl(path, records, mode="w"):
    with open(path, mode, encoding="utf-8") as handle:
        for record in records:
            handle.write(json.dumps(record, ensure_ascii=False, sort_keys=True) + "\n")


def write_json(path, value):
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(value, handle, ensure_ascii=False, indent=2)


def draft_head(draft, path, pr):
    """Returns the head a draft was found on: its stamp, else the `pr<N>-<head8>` mirror file name."""
    name = MIRROR_NAME.search(os.path.basename(path))

    if "head" in draft:
        return str(draft.get("pr")) == pr, draft["head"]

    if name:
        return name.group(1) == pr, name.group(2)

    return False, None


def prepare(drafts_path, pr, head, input_path):
    seen = set()
    prepared = []

    for draft in read_jsonl(drafts_path):
        same_pr, found = draft_head(draft, drafts_path, pr)
        key = json.dumps([draft.get(name) for name in FINDING], ensure_ascii=False)

        if not same_pr or not found or not head.startswith(found):
            print(f"draft {draft.get('path')}:{draft.get('line')} is for PR {draft.get('pr', '?')} head {found}, not #{pr} at {head}", file=sys.stderr)
            sys.exit(5)

        if key in seen:
            continue

        seen.add(key)
        record = {name: draft[name] for name in FINDING}
        record["origin"] = draft.get("origin") or {name: draft.get(name, "unknown") for name in ORIGIN}
        prepared.append({"id": f"D{len(prepared) + 1}", "pr": int(pr), "head": head, **record})

    write_jsonl(input_path, prepared)
    print(len(prepared))


def block(input_path):
    """Prints the drafts with `<` escaped so a draft's text cannot forge the end delimiter."""
    print(BEGIN)

    for draft in read_jsonl(input_path):
        claim = {"id": draft["id"], **{name: draft[name] for name in FINDING}}
        print(json.dumps(claim, ensure_ascii=False, sort_keys=True).replace("<", "\\u003c"))

    print(END)


def decisions_by_id(answer, drafts):
    """Maps each draft id to its decision; rejects unknown and duplicate ids and a draft left out."""
    known = {draft["id"] for draft in drafts}
    found = {}

    for decision in answer["decisions"]:
        ident = decision["id"]

        if ident not in known:
            raise Invalid(f"decision for unknown draft id {ident!r}: the verifier may not add findings")

        if ident in found:
            raise Invalid(f"more than one decision for draft {ident}")

        found[ident] = decision

    missing = [draft["id"] for draft in drafts if draft["id"] not in found]

    if missing:
        raise Invalid(f"no decision for draft(s) {', '.join(missing)}: every draft needs one, so a left-out draft is never treated as dropped")

    return found


def final_finding(draft, decision, visible):
    """Returns the finding to post for a kept or modified draft; its line must be in the diff."""
    source = draft if decision["decision"] == "keep" else decision
    finding = {name: source[name] for name in FINDING}

    if finding["line"] not in visible.get(finding["path"], set()):
        raise Invalid(f"{draft['id']} ({decision['decision']}) targets {finding['path']}:{finding['line']}, which is not a line of the PR diff")

    return finding


def verdicts(drafts, decisions, visible):
    """Returns the per-draft verification records and the findings to post."""
    records = []
    findings = []

    for draft in drafts:
        decision = decisions[draft["id"]]
        record = {"id": draft["id"], "draft": {name: draft[name] for name in FINDING}, "origin": draft["origin"]}

        record.update(decision=decision["decision"], reason=decision["reason"].strip() or "(no reason given)")

        if decision["decision"] != "drop":
            record["final"] = final_finding(draft, decision, visible)
            findings.append(record["final"])

        records.append(record)

    return records, findings


def origins(drafts):
    distinct = []

    for draft in drafts:
        if draft["origin"] not in distinct:
            distinct.append(draft["origin"])

    return distinct


def counts(records):
    return {name: sum(1 for record in records if record["decision"] == name) for name in ("keep", "modify", "drop")}


def apply(input_path, answer_path, diff_path, run_dir, mirror, provider, model, effort, resumed):
    drafts = read_jsonl(input_path)

    try:
        answer = load(answer_path)
    except (OSError, ValueError) as error:
        raise Invalid(f"verification answer unreadable ({error})")

    found = violation(answer, load(os.path.join(HERE, "verify-schema.json")), "verification")

    if found:
        raise Invalid(found)

    with open(diff_path, encoding="utf-8") as handle:
        visible = diff_lines(handle.read())

    records, findings = verdicts(drafts, decisions_by_id(answer, drafts), visible)
    review = {
        "verdict": "FINDINGS" if findings else "NO_FINDINGS",
        "summary": answer["summary"],
        "findings": findings,
        "unchecked": answer["unchecked"],
    }
    stages = {
        "discovery": {"origins": origins(drafts), "drafts": len(drafts), "resumed": resumed == "1"},
        "verification": {"provider": provider, "model": model, "effort": effort, **counts(records)},
    }
    missing = violation(review, load(os.path.join(HERE, "review-schema.json")), "review")

    if missing:
        raise Invalid(missing)

    tagged = [{**record, "run": run_dir, "verifier": {"provider": provider, "model": model, "effort": effort}} for record in records]

    write_jsonl(os.path.join(run_dir, "verification.jsonl"), tagged)
    write_jsonl(mirror, tagged, mode="a")
    write_json(os.path.join(run_dir, "review.json"), review)
    write_json(os.path.join(run_dir, "stages.json"), stages)
    print("verification: {keep} kept, {modify} modified, {drop} dropped".format(**counts(records)))


def done_marker(discovery_path):
    try:
        answer = load(discovery_path)
    except (OSError, ValueError) as error:
        raise Invalid(f"discovery answer unreadable ({error}); a run without drafts needs its done marker")

    found = violation(answer, load(os.path.join(HERE, "discovery-schema.json")), "discovery")

    if found:
        raise Invalid(found)

    return answer


def check_saved(answer, saved):
    if answer["drafts"] > saved:
        raise Invalid(f"the discovery done marker reports {answer['drafts']} draft(s) but only {saved} were saved; drafts may have been lost, so nothing is verified or posted")


def saved(discovery_path, drafts_path):
    try:
        answer = load(discovery_path)
    except (OSError, ValueError):
        return

    if violation(answer, load(os.path.join(HERE, "discovery-schema.json")), "discovery"):
        return

    check_saved(answer, len(read_jsonl(drafts_path)))


def empty(discovery_path, run_dir, provider, model, effort):
    answer = done_marker(discovery_path)
    check_saved(answer, 0)

    review = {"verdict": "NO_FINDINGS", "summary": answer["summary"], "findings": [], "unchecked": answer["unchecked"]}
    origin = {"provider": provider, "model": model, "effort": effort}

    write_json(os.path.join(run_dir, "review.json"), review)
    write_json(os.path.join(run_dir, "stages.json"), {"discovery": {"origins": [origin], "drafts": 0, "resumed": False}, "verification": None})


def main():
    command, *args = sys.argv[1:]
    commands = {"prepare": prepare, "block": block, "apply": apply, "empty": empty, "saved": saved}

    try:
        commands[command](*args)
    except Invalid as error:
        print(f"not posting: {error}", file=sys.stderr)
        sys.exit(3)


if __name__ == "__main__":
    main()
