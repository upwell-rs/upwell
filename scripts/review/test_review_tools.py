"""Fixture tests of the review tooling's fail-closed checks and diff parsing.

Usage: python3 -I scripts/review/test_review_tools.py
"""

import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import verify_drafts
from diffmap import diff_lines

QUOTED_DIFF = """diff --git "a/caf\\303\\251 \\"q\\"\\tx.rs" "b/caf\\303\\251 \\"q\\"\\tx.rs"
new file mode 100644
--- /dev/null
+++ "b/caf\\303\\251 \\"q\\"\\tx.rs"\t
@@ -0,0 +1,2 @@
+a
+b
diff --git a/back\\\\slash.rs b/back\\\\slash.rs
--- "a/back\\\\slash.rs"
+++ "b/back\\\\slash.rs"
@@ -3,1 +3,1 @@
-old
+new
"""

DRAFT = {"severity": "major", "path": "src/lib.rs", "line": 1, "title": "t", "body": "b", "suggestion": "s"}


def write(directory, name, value):
    path = os.path.join(directory, name)

    with open(path, "w", encoding="utf-8") as handle:
        if name.endswith(".jsonl"):
            handle.writelines(json.dumps(record) + "\n" for record in value)
        else:
            json.dump(value, handle)

    return path


def decision(ident):
    return {"id": ident, "decision": "keep", "reason": "checked", **DRAFT}


class DiffPaths(unittest.TestCase):
    """Paths git C-quotes in `+++` headers map to the files they name."""

    def test_quoted_paths_are_decoded(self):
        files = diff_lines(QUOTED_DIFF)

        self.assertEqual(files, {'café "q"\tx.rs': {1, 2}, "back\\slash.rs": {3}})


class FailClosed(unittest.TestCase):
    """Answers that could hide findings are refused instead of posted as clean."""

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.run_dir = self.directory.name

    def tearDown(self):
        self.directory.cleanup()

    def test_a_done_marker_reporting_unsaved_drafts_is_refused(self):
        marker = write(self.run_dir, "discovery.json", {"status": "DISCOVERY_DONE", "drafts": 2, "summary": "s", "unchecked": ""})

        with self.assertRaisesRegex(verify_drafts.Invalid, "reports 2 draft"):
            verify_drafts.empty(marker, self.run_dir, "codex", "m", "e")

        self.assertFalse(os.path.exists(os.path.join(self.run_dir, "review.json")))

    def test_a_done_marker_without_drafts_posts_a_clean_review(self):
        marker = write(self.run_dir, "discovery.json", {"status": "DISCOVERY_DONE", "drafts": 0, "summary": "s", "unchecked": ""})

        verify_drafts.empty(marker, self.run_dir, "codex", "m", "e")

        with open(os.path.join(self.run_dir, "review.json"), encoding="utf-8") as handle:
            self.assertEqual(json.load(handle)["verdict"], "NO_FINDINGS")

    def apply(self, decisions):
        drafts = [{"id": ident, "origin": {}, **DRAFT} for ident in ("D1", "D2")]
        inputs = write(self.run_dir, "verify-input.jsonl", drafts)
        answer = write(self.run_dir, "verify.json", {"summary": "s", "unchecked": "", "decisions": decisions})
        diff = os.path.join(self.run_dir, "pr.diff")

        with open(diff, "w", encoding="utf-8") as handle:
            handle.write("diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,1 +1,1 @@\n-x\n+y\n")

        verify_drafts.apply(inputs, answer, diff, self.run_dir, os.path.join(self.run_dir, "mirror.jsonl"), "codex", "m", "e", "0")

    def test_an_answer_leaving_out_drafts_is_refused(self):
        for decisions in ([], [decision("D1")]):
            with self.subTest(decisions=len(decisions)):
                with self.assertRaisesRegex(verify_drafts.Invalid, "no decision for draft"):
                    self.apply(decisions)

                self.assertFalse(os.path.exists(os.path.join(self.run_dir, "review.json")))

    def test_an_answer_deciding_every_draft_is_applied(self):
        self.apply([decision("D1"), decision("D2")])

        with open(os.path.join(self.run_dir, "review.json"), encoding="utf-8") as handle:
            self.assertEqual(len(json.load(handle)["findings"]), 2)


if __name__ == "__main__":
    unittest.main()
