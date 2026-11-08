"""Tees provider output to a log and, in discovery, appends each finding draft line as it arrives.

Usage: stream_drafts.py <codex|gemini|copilot> <log> <answer.json> [<drafts.jsonl> <mirror.jsonl> <stamp.json>]
Reads the provider output on stdin. Without the drafts arguments (verification) no drafts are
collected. Each saved draft is stamped with the fields of <stamp.json> (pr, head and the discovery
provider, model and effort) so a later verification can prove which head it belongs to.
For gemini (agy stream-json) the assistant text deltas are reassembled into lines and the final
structured answer is written to <answer.json>. For copilot (--output-format json, JSONL) the
assistant_message deltas are reassembled the same way and the last non-empty assistant message is
the answer. The full answer text goes to <answer stem>.response.md.
"""

import json
import os
import re
import sys

MARKER = "UPWELL_FINDING_DRAFT"
FIELDS = {"status", "severity", "path", "line", "title", "body", "suggestion"}
TEXT_FIELDS = ("path", "title", "body", "suggestion")
SEVERITIES = {"blocker", "major", "minor", "nit"}


def valid_draft(draft):
    """Accepts exactly the draft fields with their types; rejects the prompt's template line."""
    if not isinstance(draft, dict) or set(draft) != FIELDS or draft["status"] != "draft":
        return False

    line = draft["line"]
    texts = all(isinstance(draft[name], str) for name in TEXT_FIELDS)

    if not texts or not isinstance(line, int) or isinstance(line, bool) or line < 1:
        return False

    return draft["severity"] in SEVERITIES and draft["path"].strip() != "" and not draft["path"].startswith("<")


class Sink:
    """Writes the raw provider stream to the log and valid, stamped drafts to the drafts files."""

    def __init__(self, log, paths, stamp):
        self.log = open(log, "a", encoding="utf-8")
        self.paths = paths
        self.stamp = stamp
        self.seen = set()

    def raw(self, text):
        self.log.write(text)
        self.log.flush()

    def scan(self, line):
        index = line.find(MARKER)

        if not self.paths or index < 0:
            return

        try:
            draft = json.loads(line[index + len(MARKER):].strip())
        except ValueError:
            return

        if not valid_draft(draft):
            return

        key = json.dumps(draft, ensure_ascii=False, sort_keys=True)

        if key in self.seen:
            return

        self.seen.add(key)
        text = json.dumps({**draft, **self.stamp}, ensure_ascii=False, sort_keys=True)

        for path in self.paths:
            with open(path, "a", encoding="utf-8") as handle:
                handle.write(text + "\n")
                handle.flush()


def run_codex(sink):
    for line in sys.stdin:
        sink.raw(line)
        sink.scan(line)


def response_path(answer_path):
    return os.path.splitext(answer_path)[0] + ".response.md"


def fallback_answer(response):
    """Returns the last fenced json block, else the last top-level JSON object, of a response."""
    decoder = json.JSONDecoder()
    found = None

    for block in reversed(re.findall(r"```(?:json)?[ \t]*\n(.*?)```", response, re.DOTALL)):
        try:
            return json.loads(block)
        except ValueError:
            continue

    for match in re.finditer(r"^\{", response, re.MULTILINE):
        try:
            value, _ = decoder.raw_decode(response[match.start():])
        except ValueError:
            continue

        found = value

    return found


def save_response(answer_path, response):
    path = response_path(answer_path)

    with open(path, "w", encoding="utf-8") as handle:
        handle.write(response)

    return path


def write_answer(provider, answer_path, response, structured=None):
    """Saves the full response text and the final answer (structured output, else the last JSON block)."""
    saved = save_response(answer_path, response)
    answer = structured

    if answer is None:
        answer = fallback_answer(response)

        if answer is None:
            sys.exit(f"{provider} result without a structured answer; full response saved in {saved}")

        sys.stderr.write(f"{provider} gave no structured_output; using the last JSON block of {saved}\n")

    with open(answer_path, "w", encoding="utf-8") as handle:
        json.dump(answer, handle)


def scan_lines(sink, buffers, key, delta):
    buffers[key] = buffers.get(key, "") + delta
    *complete, buffers[key] = buffers[key].split("\n")

    for text in complete:
        sink.scan(text)


def run_gemini(sink, answer_path):
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

            if update.get("text_delta"):
                scan_lines(sink, buffers, step, update["text_delta"])

            if update.get("state") == "DONE" and step in buffers:
                sink.scan(buffers.pop(step))

        elif event.get("event") == "result":
            final = event["result"]

    for text in buffers.values():
        sink.scan(text)

    if final is None:
        sys.exit("agy ended without a result event")

    response = final.get("response") or ""

    if final.get("status") != "SUCCESS":
        saved = save_response(answer_path, response)
        sys.exit(f"agy result was not successful (status {final.get('status')}); full response saved in {saved}")

    write_answer("agy", answer_path, response, structured=final.get("structured_output"))


def run_copilot(sink, answer_path):
    buffers = {}
    response = ""
    result = None

    for line in sys.stdin:
        sink.raw(line)

        try:
            event = json.loads(line)
        except ValueError:
            continue

        data = event.get("data") or {}
        kind = event.get("type")

        if kind == "assistant.message_delta" and data.get("deltaContent"):
            scan_lines(sink, buffers, data.get("messageId"), data["deltaContent"])
        elif kind == "assistant.message":
            sink.scan(buffers.pop(data.get("messageId"), ""))
            content = data.get("content") or ""

            if content and not data.get("toolRequests"):
                response = content

        elif kind == "result":
            result = event

    for text in buffers.values():
        sink.scan(text)

    if result is None:
        sys.exit("copilot ended without a result event")

    if result.get("exitCode") != 0:
        sys.exit(f"copilot result was not successful (exit code {result.get('exitCode')})")

    write_answer("copilot", answer_path, response)


def main():
    mode, log, answer_path = sys.argv[1:4]
    paths = sys.argv[4:6]
    stamp = {}

    if len(sys.argv) > 6:
        with open(sys.argv[6], encoding="utf-8") as handle:
            stamp = json.load(handle)

    sink = Sink(log, paths, stamp)

    if mode == "codex":
        run_codex(sink)
    elif mode == "copilot":
        run_copilot(sink, answer_path)
    else:
        run_gemini(sink, answer_path)


main()
