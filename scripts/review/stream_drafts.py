"""Tees provider output to a log and appends each finding draft line to drafts files as it arrives.

Usage: stream_drafts.py <codex|gemini> <log> <drafts.jsonl> <mirror.jsonl> <review.json>
Reads the provider output on stdin. For gemini (agy stream-json) the assistant text deltas are
reassembled into lines and the final structured answer is written to <review.json>.
"""

import json
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
    if final.get("status") != "SUCCESS" or "structured_output" not in final:
        sys.exit(f"agy result without a structured answer (status {final.get('status')})")
    with open(review_path, "w", encoding="utf-8") as handle:
        json.dump(final["structured_output"], handle)


def main():
    mode, log, drafts, mirror, review_path = sys.argv[1:6]
    sink = Sink(log, drafts, mirror)
    if mode == "codex":
        run_codex(sink)
    else:
        run_gemini(sink, review_path)


main()
