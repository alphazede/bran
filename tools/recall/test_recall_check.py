"""Issue #60 (DES-06): recall_check.py measured on the synthetic recall fixture.

Run with a built bran: BRAN_BIN=<binary> python3 -m unittest tools/recall/test_recall_check.py
(default binary: target/debug/bran).
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / "tools/recall/recall_check.py"
FIXTURE = REPO / "fixtures/recall"


def default_bran_bin() -> Path:
    if env_bin := os.environ.get("BRAN_BIN"):
        return Path(env_bin)
    target_dir = os.environ.get("CARGO_TARGET_DIR")
    base = Path(target_dir) if target_dir else REPO / "target"
    if not base.is_absolute():
        base = REPO / base
    if target := os.environ.get("CARGO_BUILD_TARGET"):
        return base / target / "debug/bran"
    return base / "debug/bran"


BRAN = default_bran_bin()

# Hand-derived from fixtures/recall: questions 1-8 name their file in path,
# title and body; question 9 is outranked by the archived note; question 10
# shares no term with its expected file.
RANKS = [1, 1, 1, 1, 1, 1, 1, 1, 2, None]
CLASSES = {
    "doc": {"questions": 6, "top1": 4, "top5": 5, "misses": 1},
    "runbook": {"questions": 2, "top1": 2, "top5": 2, "misses": 0},
    "reference": {"questions": 1, "top1": 1, "top5": 1, "misses": 0},
    "unclassified": {"questions": 1, "top1": 1, "top5": 1, "misses": 0},
}


def make_root(parent):
    root = Path(parent) / "root"
    shutil.copytree(FIXTURE / "tree", root)
    (root / ".bran").mkdir()
    shutil.copy(FIXTURE / "policy.yaml", root / ".bran/policy.yaml")
    return root


def check(root, questions, *extra):
    return subprocess.run(
        [
            sys.executable,
            "-I",
            str(SCRIPT),
            "--bran",
            str(BRAN),
            "--root",
            str(root),
            "--questions",
            str(questions),
            *extra,
        ],
        capture_output=True,
        text=True,
        timeout=300,
        check=False,
    )


class RecallCheckTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not BRAN.is_file():
            raise AssertionError(f"build bran first or set BRAN_BIN: {BRAN} missing")

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = make_root(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def summary(self, mode, *extra, questions=FIXTURE / "questions.jsonl", root=None):
        run = check(root or self.root, questions, "--mode", mode, "--json", *extra)
        self.assertEqual(run.returncode, 0, run.stderr)
        return json.loads(run.stdout)

    def assert_fixture_metrics(self, summary, mode):
        self.assertEqual(summary["mode"], mode)
        self.assertEqual(summary["questions"], 10)
        self.assertEqual(
            (summary["top1"], summary["top5"], summary["misses"]), (8, 9, 1)
        )
        self.assertEqual([q["rank"] for q in summary["per_question"]], RANKS)
        self.assertEqual(summary["classes"], CLASSES)
        for key in ("latency_seconds", "output_chars"):
            median, worst = summary[key]["median"], summary[key]["worst"]
            self.assertGreaterEqual(worst, median, key)
            self.assertGreater(worst, 0, key)

    def test_full_mode_reads_source_rankings(self):
        self.assert_fixture_metrics(self.summary("full", "--repeat", "2"), "full")

    def test_brief_mode_reads_results(self):
        summary = self.summary("brief")
        self.assert_fixture_metrics(summary, "brief")
        self.assertLessEqual(summary["output_chars"]["worst"], 2000)
        self.assertEqual(summary["size_findings"], [])

    def test_table_lists_the_metrics(self):
        run = check(self.root, FIXTURE / "questions.jsonl", "--mode", "full")
        self.assertEqual(run.returncode, 0, run.stderr)
        lines = run.stdout.splitlines()
        for expected in ("top-1: 8/10", "top-5: 9/10", "misses: 1"):
            self.assertIn(expected, lines)

    def test_question_that_errors_counts_as_miss(self):
        questions = Path(self.tmp.name) / "errors.jsonl"
        questions.write_text(
            '{"question": "   ", "expected": "docs/valve-wiring.md"}\n'
            '{"question": "valve relay wiring", "expected": "docs/valve-wiring.md"}\n'
        )
        summary = self.summary("full", questions=questions)
        self.assertEqual((summary["top1"], summary["misses"]), (1, 1))
        failed = summary["per_question"][0]
        self.assertIsNone(failed["rank"])
        self.assertTrue(failed["error"])

    def test_unreadable_questions_file_exits_nonzero_naming_it(self):
        missing = Path(self.tmp.name) / "absent.jsonl"
        run = check(self.root, missing, "--mode", "full")
        self.assertNotEqual(run.returncode, 0)
        self.assertIn(str(missing), run.stderr)

    def test_long_exact_paths_are_listed_as_size_finding(self):
        root = Path(self.tmp.name) / "long"
        (root / ".bran").mkdir(parents=True)
        shutil.copy(FIXTURE / "policy.yaml", root / ".bran/policy.yaml")
        path_chars = 0
        for n in range(8):
            segments = [f"segment-{n}-{k}-" + "x" * 48 for k in range(5)]
            path = Path("long", *segments, f"lantern-{n}.md")
            (root / path.parent).mkdir(parents=True)
            (root / path).write_text(
                f"---\ntype: concept\ntitle: Lantern {n}\nokf_status: active\n"
                f"tags: [developer]\n---\n\n# Lantern {n}\n\nThe lantern note {n}.\n"
            )
            path_chars += len(str(path))
        questions = Path(self.tmp.name) / "long.jsonl"
        questions.write_text(
            json.dumps({"question": "lantern", "expected": str(path)}) + "\n"
        )
        summary = self.summary("brief", questions=questions, root=root)
        [finding] = summary["size_findings"]
        self.assertEqual(finding["question"], "lantern")
        self.assertEqual(finding["path_chars"], path_chars)
        self.assertGreater(finding["output_chars"], 2000)

    def test_repeat_records_worst_successful_response_in_size_findings(self):
        state = Path(self.tmp.name) / "calls.txt"
        wrapper = Path(self.tmp.name) / "fake_bran.py"
        long_path = "x/" * 100 + "hit.md"
        first_payload = json.dumps({"data": {"results": [{"path": "a.md"}]}})
        padding_len = 2584 - len(
            json.dumps({"data": {"results": [{"path": long_path, "reason": ""}]}})
        )
        second_payload = json.dumps(
            {"data": {"results": [{"path": long_path, "reason": "r" * padding_len}]}}
        )
        wrapper.write_text(
            "#!/usr/bin/env python3\n"
            "import pathlib, sys\n"
            f"state = pathlib.Path({str(state)!r})\n"
            "count = int(state.read_text()) if state.exists() else 0\n"
            "state.write_text(str(count + 1))\n"
            f"payloads = [{first_payload!r}, {second_payload!r}]\n"
            "print(payloads[min(count, len(payloads) - 1)])\n",
            encoding="utf-8",
        )
        wrapper.chmod(0o755)
        questions = Path(self.tmp.name) / "repeat_size.jsonl"
        questions.write_text(
            json.dumps({"question": "q", "expected": long_path}) + "\n",
            encoding="utf-8",
        )
        run = subprocess.run(
            [
                sys.executable,
                "-I",
                str(SCRIPT),
                "--bran",
                str(wrapper),
                "--root",
                str(self.root),
                "--questions",
                str(questions),
                "--mode",
                "brief",
                "--repeat",
                "2",
                "--json",
            ],
            capture_output=True,
            text=True,
            timeout=30,
            check=False,
        )
        self.assertEqual(run.returncode, 0, run.stderr)
        summary = json.loads(run.stdout)
        self.assertEqual(summary["output_chars"]["worst"], len(second_payload))
        self.assertEqual(
            summary["per_question"][0]["output_chars"], len(second_payload)
        )
        [finding] = summary["size_findings"]
        self.assertEqual(finding["output_chars"], len(second_payload))
        self.assertEqual(finding["path_chars"], len(long_path))


if __name__ == "__main__":
    unittest.main()
