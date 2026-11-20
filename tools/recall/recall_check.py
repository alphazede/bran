#!/usr/bin/env python3
"""Evaluate BRAN query recall, latency, and output size against a JSONL question set."""

from __future__ import annotations

import argparse
import json
import re
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path

# Brief output stays inside its character budget by dropping whole results and
# warning with this text (README "Use --top N").
BRIEF_TRUNCATED = re.compile(r"brief_truncated: (\d+) of (\d+) results omitted")
BRIEF_CHAR_LIMIT = 2000
QUERY_TIMEOUT_SECONDS = 605


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run BRAN queries for a JSONL question set and report recall metrics."
    )
    parser.add_argument("--bran", required=True, help="Path to the bran binary.")
    parser.add_argument("--root", required=True, help="Repository root to query.")
    parser.add_argument(
        "--questions", required=True, help="Path to the JSONL question set."
    )
    parser.add_argument(
        "--mode",
        choices=("brief", "full"),
        default="brief",
        help="Query mode: brief (--brief, reads data.results) or full (default query, reads data.source_rankings).",
    )
    parser.add_argument(
        "--repeat",
        type=int,
        default=1,
        help="Number of times to execute each question.",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Emit only the JSON summary to stdout.",
    )
    return parser.parse_args(argv)


def load_questions(questions_path: str) -> list[dict[str, object]]:
    path = Path(questions_path)
    try:
        raw_lines = path.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeDecodeError) as exc:
        print(
            f"error: unable to read questions file {questions_path}: {exc}",
            file=sys.stderr,
        )
        sys.exit(1)

    rows: list[dict[str, object]] = []
    for line_no, line in enumerate(raw_lines, start=1):
        stripped = line.strip()
        if not stripped:
            continue
        try:
            row = json.loads(stripped)
        except json.JSONDecodeError as exc:
            print(
                f"error: invalid JSON in questions file {questions_path}:{line_no}: {exc}",
                file=sys.stderr,
            )
            sys.exit(1)
        if (
            not isinstance(row, dict)
            or not isinstance(row.get("question"), str)
            or not isinstance(row.get("expected"), str)
        ):
            print(
                f"error: invalid question row in {questions_path}:{line_no}: expected string fields 'question' and 'expected'",
                file=sys.stderr,
            )
            sys.exit(1)
        rows.append(row)
    return rows


def resolve_bran(bran_arg: str) -> str:
    candidate = Path(bran_arg)
    if candidate.is_file():
        return str(candidate)
    if "/" not in bran_arg and "\\" not in bran_arg:
        found = shutil.which(bran_arg)
        if found:
            return found
    print(f"error: bran binary not found: {bran_arg}", file=sys.stderr)
    sys.exit(1)


def truncation_notice(payload: dict[str, object]) -> tuple[int, int] | None:
    """Return (omitted, total) from a `brief_truncated` warning, if present."""
    warnings = payload.get("warnings")
    if not isinstance(warnings, list):
        return None
    for warning in warnings:
        if isinstance(warning, str):
            match = BRIEF_TRUNCATED.match(warning)
            if match:
                return int(match[1]), int(match[2])
    return None


def extract_rank(
    stdout: str,
    mode: str,
    expected: str,
) -> tuple[int | None, int, tuple[int, int] | None, str | None]:
    try:
        payload = json.loads(stdout)
    except json.JSONDecodeError as exc:
        return None, 0, None, f"invalid JSON output: {exc}"

    if not isinstance(payload, dict):
        return None, 0, None, "query output JSON root is not an object"
    data = payload.get("data")
    if not isinstance(data, dict):
        return None, 0, None, "query output missing 'data' object"

    list_key = "results" if mode == "brief" else "source_rankings"
    path_key = "path" if mode == "brief" else "locator"
    items = data.get(list_key)
    if not isinstance(items, list):
        return None, 0, None, f"query output missing 'data.{list_key}' array"

    rank: int | None = None
    path_chars = 0
    for idx, item in enumerate(items, start=1):
        if not isinstance(item, dict):
            continue
        item_path = item.get(path_key)
        if isinstance(item_path, str):
            path_chars += len(item_path)
            if rank is None and item_path == expected:
                rank = idx
    return rank, path_chars, truncation_notice(payload), None


def run_once(
    bran_bin: str,
    root: str,
    question: str,
    expected: str,
    mode: str,
) -> tuple[int | None, str | None, float, int, int, tuple[int, int] | None]:
    cmd = [bran_bin, "query", root]
    if mode == "brief":
        cmd.append("--brief")
    cmd.append(question)

    started = time.perf_counter()
    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            check=False,
            timeout=QUERY_TIMEOUT_SECONDS,
        )
    except subprocess.TimeoutExpired:
        elapsed = time.perf_counter() - started
        return (
            None,
            f"query timed out after {QUERY_TIMEOUT_SECONDS} s",
            elapsed,
            0,
            0,
            None,
        )
    elapsed = time.perf_counter() - started
    out_chars = len(proc.stdout.rstrip("\n"))

    if proc.returncode != 0:
        err = (
            proc.stderr.strip() or proc.stdout.strip() or f"exit code {proc.returncode}"
        )
        return None, err, elapsed, out_chars, 0, None

    rank, path_chars, truncated, parse_err = extract_rank(proc.stdout, mode, expected)
    return rank, parse_err, elapsed, out_chars, path_chars, truncated


def evaluate(
    bran_bin: str,
    root: str,
    rows: list[dict[str, object]],
    mode: str,
    repeat: int,
) -> dict[str, object]:
    top1 = 0
    top5 = 0
    misses = 0
    all_latencies: list[float] = []
    all_output_chars: list[int] = []
    classes: dict[str, dict[str, int]] = {}
    per_question: list[dict[str, object]] = []
    size_findings: list[dict[str, object]] = []

    for row in rows:
        question = str(row["question"])
        expected = str(row["expected"])
        raw_class = row.get("class")
        q_class = (
            raw_class if isinstance(raw_class, str) and raw_class else "unclassified"
        )

        q_latencies: list[float] = []
        q_chars: list[int] = []
        rep_rank: int | None = None
        rep_err: str | None = None
        rep_out_chars = 0
        worst_ok_out_chars: int | None = None
        worst_ok_path_chars = 0
        worst_ok_trunc: tuple[int, int] | None = None
        worst_trunc: tuple[int, int, int, int] | None = None

        for rep in range(repeat):
            rank, err, elapsed, out_chars, path_chars, truncated = run_once(
                bran_bin, root, question, expected, mode
            )
            q_latencies.append(elapsed)
            q_chars.append(out_chars)
            all_latencies.append(elapsed)
            all_output_chars.append(out_chars)
            if rep == 0:
                rep_rank = rank
                rep_err = err
                rep_out_chars = out_chars
            if err is None and (
                worst_ok_out_chars is None or out_chars > worst_ok_out_chars
            ):
                worst_ok_out_chars = out_chars
                worst_ok_path_chars = path_chars
                worst_ok_trunc = truncated
            if (
                err is None
                and truncated
                and (worst_trunc is None or truncated[0] > worst_trunc[0])
            ):
                worst_trunc = (*truncated, path_chars, out_chars)

        bucket = classes.setdefault(
            q_class,
            {"questions": 0, "top1": 0, "top5": 0, "misses": 0},
        )
        bucket["questions"] += 1
        if rep_rank is None:
            misses += 1
            bucket["misses"] += 1
        else:
            if rep_rank == 1:
                top1 += 1
                bucket["top1"] += 1
            if rep_rank <= 5:
                top5 += 1
                bucket["top5"] += 1

        reported_out_chars = (
            worst_ok_out_chars if worst_ok_out_chars is not None else rep_out_chars
        )
        per_question.append(
            {
                "question": question,
                "expected": expected,
                "class": q_class,
                "rank": rep_rank,
                "error": rep_err,
                "latency_seconds": statistics.median(q_latencies),
                "output_chars": reported_out_chars,
            }
        )

        if (
            mode == "brief"
            and worst_ok_out_chars is not None
            and worst_ok_out_chars > BRIEF_CHAR_LIMIT
        ):
            omitted, total = worst_ok_trunc or (0, 0)
            size_findings.append(
                {
                    "question": question,
                    "output_chars": worst_ok_out_chars,
                    "path_chars": worst_ok_path_chars,
                    "omitted": omitted,
                    "total": total,
                }
            )
        elif mode == "brief" and worst_trunc is not None:
            omitted, total, path_chars, out_chars = worst_trunc
            size_findings.append(
                {
                    "question": question,
                    "output_chars": out_chars,
                    "path_chars": path_chars,
                    "omitted": omitted,
                    "total": total,
                }
            )

    return {
        "mode": mode,
        "questions": len(rows),
        "top1": top1,
        "top5": top5,
        "misses": misses,
        "latency_seconds": {
            "median": statistics.median(all_latencies) if all_latencies else 0.0,
            "worst": max(all_latencies) if all_latencies else 0.0,
        },
        "output_chars": {
            "median": statistics.median(all_output_chars) if all_output_chars else 0,
            "worst": max(all_output_chars) if all_output_chars else 0,
        },
        "classes": classes,
        "per_question": per_question,
        "size_findings": size_findings,
    }


def print_table(summary: dict[str, object]) -> None:
    total = summary["questions"]
    print(f"mode: {summary['mode']}")
    print(f"questions: {total}")
    print(f"top-1: {summary['top1']}/{total}")
    print(f"top-5: {summary['top5']}/{total}")
    print(f"misses: {summary['misses']}")
    latency = summary["latency_seconds"]
    chars = summary["output_chars"]
    print(
        f"latency_seconds: median={latency['median']:.4f} worst={latency['worst']:.4f}"
    )
    print(f"output_chars: median={chars['median']} worst={chars['worst']}")
    for cls_name, stats in summary["classes"].items():
        print(
            f"class {cls_name}: questions={stats['questions']} "
            f"top-1={stats['top1']} top-5={stats['top5']} misses={stats['misses']}"
        )
    findings = summary["size_findings"]
    if findings:
        print(f"size_findings: {len(findings)}")


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    if args.repeat < 1:
        print("error: --repeat must be >= 1", file=sys.stderr)
        return 2

    rows = load_questions(args.questions)
    bran_bin = resolve_bran(args.bran)
    if not Path(args.root).is_dir():
        print(f"error: root directory not found: {args.root}", file=sys.stderr)
        return 1

    summary = evaluate(bran_bin, args.root, rows, args.mode, args.repeat)
    if args.json:
        print(json.dumps(summary, indent=2))
    else:
        print_table(summary)
    return 0


if __name__ == "__main__":
    sys.exit(main())
