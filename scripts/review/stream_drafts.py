"""Tees provider output to a log and appends each finding draft line to drafts files as it arrives.

Usage: stream_drafts.py <codex|gemini> <log> <drafts.jsonl> <mirror.jsonl> <review.json>
Reads the provider output on stdin. For gemini (agy stream-json) the assistant text deltas are
reassembled into lines and the final structured answer is written to <review.json>.
"""

import json
import os
import re
import sys

MARKER = "UPWELL_FINDING_DRAFT"
FIELDS = {"status", "severity", "path", "line", "title", "body", "suggestion"}


class Sink:
    def __init__(self, log, drafts, mirror):
        self.log = open(log, "a", encoding="utf-8")
        self.paths = [drafts, mirror]
        self.seen = set()

    def raw(self, text):
        self.log.write(text)
        self.log.flush()

    def scan(self, line):
        index = line.find(MARKER)
        if index < 0:
            return
        try:
            draft = json.loads(line[index + len(MARKER):].strip())
        except ValueError:
            return
        if not isinstance(draft, dict) or draft.get("status") != "draft" or not FIELDS.issuperset(draft):
            return
        line_no = draft.get("line")
        if not isinstance(line_no, int) or line_no < 1 or str(draft.get("path", "<")).startswith("<"):
            return
        text = json.dumps(draft, ensure_ascii=False, sort_keys=True)
        if text in self.seen:
            return
        self.seen.add(text)
        for path in self.paths:
            with open(path, "a", encoding="utf-8") as handle:
                handle.write(text + "\n")
                handle.flush()


def run_codex(sink):
    for line in sys.stdin:
        sink.raw(line)
        sink.scan(line)


def fallback_answer(response):
    """Returns the last fenced json block, else the last top-level JSON object, of an agy response."""
    for block in reversed(re.findall(r"```(?:json)?[ \t]*\n(.*?)```", response, re.DOTALL)):
        try:
            return json.loads(block)
        except ValueError:
            continue
    decoder = json.JSONDecoder()
    found = None
    for match in re.finditer(r"^\{", response, re.MULTILINE):
        try:
            value, _ = decoder.raw_decode(response[match.start():])
        except ValueError:
            continue
        found = value
    return found


def run_gemini(sink, review_path):
    buffers = {}
    final = None
    for line in sys.stdin:
        sink.raw(line)
        try:
            event = json.loads(line)
        except ValueError:
            continue
        if event.get("event") == "step_update":
            update = event["step_update"]
            step = update.get("step_index")
            delta = update.get("text_delta")
            if delta:
                buffers[step] = buffers.get(step, "") + delta
                *complete, buffers[step] = buffers[step].split("\n")
                for text in complete:
                    sink.scan(text)
            if update.get("state") == "DONE" and step in buffers:
                sink.scan(buffers.pop(step))
        elif event.get("event") == "result":
            final = event["result"]
    for text in buffers.values():
        sink.scan(text)
    if final is None:
        sys.exit("agy ended without a result event")
    response = final.get("response") or ""
    response_path = os.path.join(os.path.dirname(review_path), "response.md")
    with open(response_path, "w", encoding="utf-8") as handle:
        handle.write(response)
    if final.get("status") != "SUCCESS":
        sys.exit(f"agy result was not successful (status {final.get('status')}); full response saved in {response_path}")
    answer = final.get("structured_output")
    if answer is None:
        answer = fallback_answer(response)
        if answer is None:
            sys.exit(f"agy result without a structured answer; full response saved in {response_path}")
        sys.stderr.write(f"agy gave no structured_output; using the last JSON block of {response_path}\n")
    with open(review_path, "w", encoding="utf-8") as handle:
        json.dump(answer, handle)


def main():
    mode, log, drafts, mirror, review_path = sys.argv[1:6]
    sink = Sink(log, drafts, mirror)
    if mode == "codex":
        run_codex(sink)
    else:
        run_gemini(sink, review_path)


main()
