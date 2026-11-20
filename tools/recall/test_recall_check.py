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
from unittest.mock import patch

if __package__:
    from . import recall_check
else:
    import recall_check

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
        path_lengths = []
        for n in range(8):
            segments = [f"segment-{n}-{k}-" + "x" * 48 for k in range(5)]
            path = Path("long", *segments, f"lantern-{n}.md")
            (root / path.parent).mkdir(parents=True)
            (root / path).write_text(
                f"---\ntype: concept\ntitle: Lantern {n}\nokf_status: active\n"
                f"tags: [developer]\n---\n\n# Lantern {n}\n\nThe lantern note {n}.\n"
            )
            path_lengths.append(len(str(path)))
        questions = Path(self.tmp.name) / "long.jsonl"
        questions.write_text(
            json.dumps({"question": "lantern", "expected": str(path)}) + "\n"
        )
        summary = self.summary("brief", questions=questions, root=root)
        # The brief budget wins: whole results are omitted (README, "Use --top
        # N"), so the finding reports the omission, not an over-budget output.
        [finding] = summary["size_findings"]
        self.assertEqual(finding["question"], "lantern")
        self.assertEqual(finding["total"], 8)
        self.assertGreaterEqual(finding["omitted"], 1)
        kept = finding["total"] - finding["omitted"]
        # Equal scores tie-break on path, so the first `kept` paths are listed.
        self.assertEqual(finding["path_chars"], sum(path_lengths[:kept]))
        self.assertLessEqual(finding["output_chars"], 2000)

    def test_repeat_records_worst_successful_response_in_size_findings(self):
        long_path = "x/" * 100 + "hit.md"
        first_payload = json.dumps({"data": {"results": [{"path": "a.md"}]}})
        padding_len = 2584 - len(
            json.dumps({"data": {"results": [{"path": long_path, "reason": ""}]}})
        )
        second_payload = json.dumps(
            {"data": {"results": [{"path": long_path, "reason": "r" * padding_len}]}}
        )
        summary = self.repeat_outputs(first_payload, second_payload, long_path)
        self.assertEqual(summary["output_chars"]["worst"], len(second_payload))
        self.assertEqual(
            summary["per_question"][0]["output_chars"], len(second_payload)
        )
        [finding] = summary["size_findings"]
        self.assertEqual(finding["output_chars"], len(second_payload))
        self.assertEqual(finding["path_chars"], len(long_path))
        self.assertEqual(finding["omitted"], 0)

    def test_repeat_size_finding_uses_a_single_response(self):
        truncated = json.dumps(
            {
                "data": {"results": [{"path": "a.md"}]},
                "warnings": ["brief_truncated: 2 of 3 results omitted"],
            }
        )
        complete = json.dumps({"data": {"results": [{"path": "x" * 250 + ".md"}]}})
        [finding] = self.repeat_outputs(truncated, complete, "a.md")["size_findings"]
        self.assertEqual(finding["output_chars"], len(truncated))
        self.assertEqual(finding["path_chars"], len("a.md"))
        self.assertEqual(finding["omitted"], 2)

    def test_repeat_size_finding_reports_oversize_even_if_another_reply_truncated(self):
        truncated = json.dumps(
            {
                "data": {"results": [{"path": "a.md"}]},
                "warnings": ["brief_truncated: 2 of 3 results omitted"],
            }
        )
        oversized = json.dumps({"data": {"results": [{"path": "x" * 2100 + ".md"}]}})
        [finding] = self.repeat_outputs(truncated, oversized, "a.md")["size_findings"]
        self.assertEqual(finding["output_chars"], len(oversized))
        self.assertEqual(finding["path_chars"], 2103)
        self.assertEqual(finding["omitted"], 0)

    def repeat_outputs(self, first_payload, second_payload, expected):
        state = Path(self.tmp.name) / "calls.txt"
        wrapper = Path(self.tmp.name) / "fake_bran.py"
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
            json.dumps({"question": "q", "expected": expected}) + "\n",
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
        return json.loads(run.stdout)

    def test_query_timeout_is_reported(self):
        wrapper = Path(self.tmp.name) / "slow_bran.py"
        wrapper.write_text("#!/usr/bin/env python3\nimport time\ntime.sleep(0.5)\n")
        wrapper.chmod(0o755)
        with patch.object(recall_check, "QUERY_TIMEOUT_SECONDS", 0.05, create=True):
            rank, error, _, _, _, _ = recall_check.run_once(
                str(wrapper), str(self.root), "quillwort", "a.md", "brief"
            )
        self.assertIsNone(rank)
        self.assertIn("timed out", error)


if __name__ == "__main__":
    unittest.main()
