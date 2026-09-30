#!/usr/bin/env python3
"""Prove the synthetic fixture database and the SQLite engine guard, offline.

Standard library only: Python's sqlite3 is the engine. The database is built
in a temporary directory from fixtures/database/synthetic.sql, so no server,
account, or network is involved. See docs/database-boundary.md.
"""

from __future__ import annotations

import hashlib
import shutil
import sqlite3
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "fixtures" / "database"
ALLOWED_TABLES = frozenset({"customers", "orders", "order_totals"})
# Mirrors ALLOWED_FUNCTIONS in crates/bran-core/src/database/sql.rs.
ALLOWED_FUNCTIONS = frozenset(
    {"abs", "avg", "coalesce", "count", "length", "lower", "max", "min", "round", "sum", "upper"}
)
REFUSED_CLASSES = frozenset(
    {
        "write",
        "multi-statement",
        "session",
        "procedure",
        "unsafe-function",
        "external-link",
        "compound",
        "secret-reflection",
        "policy",
    }
)


def fail(message: str) -> int:
    print(f"FAIL database contract: {message}")
    return 1


def build(path: Path) -> None:
    connection = sqlite3.connect(path)
    try:
        connection.executescript((FIXTURES / "synthetic.sql").read_text(encoding="utf-8"))
        connection.commit()
    finally:
        connection.close()


def dump_digest(path: Path) -> str:
    connection = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    try:
        text = "\n".join(connection.iterdump())
    finally:
        connection.close()
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def authorize(action: int, first: str | None, second: str | None, _db: str | None, _via: str | None) -> int:
    if action == sqlite3.SQLITE_SELECT:
        return sqlite3.SQLITE_OK
    if action == sqlite3.SQLITE_READ and first in ALLOWED_TABLES:
        return sqlite3.SQLITE_OK
    if action == sqlite3.SQLITE_FUNCTION and second in ALLOWED_FUNCTIONS:
        return sqlite3.SQLITE_OK
    return sqlite3.SQLITE_DENY


def guarded(path: Path) -> sqlite3.Connection:
    """The engine-level guard a SQLite evidence adapter must apply."""
    connection = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    connection.execute("PRAGMA query_only = ON")
    connection.set_authorizer(authorize)
    return connection


def corpus() -> list[tuple[str, str, str]]:
    """Rows are class, engine effect, exact Rust gate rejection, SQL."""
    rows = []
    for number, line in enumerate((FIXTURES / "queries.tsv").read_text(encoding="utf-8").splitlines(), 1):
        fields = line.split("\t")
        if len(fields) != 4 or not all(fields) or (fields[0] == "allowed") != (fields[2] == "-"):
            raise ValueError(f"queries.tsv line {number} is not class<TAB>effect<TAB>rejection<TAB>sql")
        rows.append((fields[0], fields[1], fields[3]))
    return rows


def main() -> int:
    try:
        entries = corpus()
    except (OSError, ValueError) as error:
        return fail(str(error))
    with tempfile.TemporaryDirectory(prefix="bran-database-contract-") as scratch:
        first, second = Path(scratch, "first.db"), Path(scratch, "second.db")
        build(first)
        build(second)
        baseline = dump_digest(first)
        if dump_digest(second) != baseline:
            return fail("building the fixture twice gave different databases")

        connection = sqlite3.connect(f"file:{first}?mode=ro", uri=True)
        try:
            people = connection.execute("SELECT name, email FROM customers").fetchall()
        finally:
            connection.close()
        if not people or not all(
            name.startswith("Synthetic") and email.endswith("@example.invalid") for name, email in people
        ):
            return fail("fixture rows must be synthetic names at the reserved .invalid domain")

        allowed = refused = gate_only = mutating = 0
        for kind, effect, sql in entries:
            if kind != "allowed" and kind not in REFUSED_CLASSES:
                return fail(f"unknown class {kind!r}")
            if kind == "allowed" or effect == "gate-only":
                connection = guarded(first)
                try:
                    rows = connection.execute(sql, (1000,) * sql.count("?")).fetchall()
                except sqlite3.Error as error:
                    return fail(f"guard refused an accepted query ({error}): {sql}")
                finally:
                    connection.close()
                if kind == "allowed" and not rows:
                    return fail(f"allowed query returned no rows: {sql}")
                allowed += kind == "allowed"
                gate_only += effect == "gate-only"
                continue
            connection = guarded(first)
            try:
                connection.execute(sql).fetchall()
            except (sqlite3.Error, sqlite3.Warning):
                refused += 1
            else:
                return fail(f"guard executed a {kind} statement: {sql}")
            finally:
                connection.close()
            if effect == "mutates":
                copy = Path(scratch, "unguarded.db")
                shutil.copyfile(first, copy)
                unguarded = sqlite3.connect(copy)
                try:
                    unguarded.executescript(sql)
                    unguarded.commit()
                finally:
                    unguarded.close()
                if dump_digest(copy) == baseline:
                    return fail(f"statement marked mutates changed nothing unguarded: {sql}")
                mutating += 1
            elif effect != "-":
                return fail(f"unknown effect {effect!r}")
        if dump_digest(first) != baseline:
            return fail("guarded execution changed the database")

        # Timeout and cancellation: a progress handler interrupts a long query.
        connection = guarded(first)
        calls = 0

        def stop_after_budget() -> int:
            nonlocal calls
            calls += 1
            return int(calls > 50)

        connection.set_progress_handler(stop_after_budget, 1)
        try:
            connection.execute(
                "SELECT count(*) FROM main.orders AS a, main.orders AS b, main.orders AS c, "
                "main.customers AS d ORDER BY 1"
            ).fetchall()
        except sqlite3.OperationalError as error:
            if "interrupted" not in str(error):
                return fail(f"progress handler raised an unexpected error: {error}")
        else:
            return fail("progress handler did not interrupt the query")
        finally:
            connection.close()

    print(
        f"PASS database contract: allowed={allowed} refused={refused} "
        f"gate_only={gate_only} mutating={mutating} interrupt=ok"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
