#!/usr/bin/env python3
"""Hold every schema contraction to a record that an earlier release stopped
using what it removes (ADR 0047).

Each module's database is built the way the control plane builds it: the
shared migrations of `panel-sqlite` and the module's own, in version order,
applied to an empty SQLite database. After each file the schema is compared
with the one before. Adding tables, columns or nullable fields expands a
schema and always passes; a table rebuilt with all its columns passes too.
These changes contract it, because the previous release would fail on them:

* `drop table T` and `drop column T.C`, renames included;
* `retype column T.C`, a changed declared type;
* `require column T.C`, an existing column that becomes NOT NULL;
* `require new column T.C`, a NOT NULL column without a default, which the
  previous release's inserts do not name.

A contraction passes only when `.github/policies/migration-contractions.json`
records it, with the release from which nothing used what it removes; a
record that matches no contraction fails, so none outlives its migration.
CHECK constraints are not compared.
"""

from __future__ import annotations

import json
import re
import sqlite3
import sys
from dataclasses import dataclass
from pathlib import Path

from policy import PolicyError, fields, owners

COMMON = "panel-sqlite"
POLICY = ".github/policies/migration-contractions.json"
MIGRATION = re.compile(r"(?P<version>[0-9]+)_[A-Za-z0-9_]+\.sql")
RELEASE = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")
RECORD_FIELDS = {"module", "migration", "change", "unused_since", "owner", "reason"}


@dataclass(frozen=True, order=True)
class Contraction:
    module: str
    migration: str
    change: str


def migrations(directory: Path) -> list[Path]:
    files = []
    for path in sorted(directory.glob("*.sql")):
        if not MIGRATION.fullmatch(path.name):
            raise PolicyError(f"{path}: migration names start with a version and an underscore")
        files.append(path)
    return files


def schema(database: sqlite3.Connection) -> dict[str, dict[str, tuple[str, int, object]]]:
    tables = {}
    for (name,) in database.execute(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'"
    ):
        tables[name] = {
            column: (declared.upper(), notnull, default)
            for _, column, declared, notnull, default, _ in database.execute(
                f'PRAGMA table_info("{name}")'
            )
        }
    return tables


def compare(before: dict, after: dict) -> list[str]:
    changes = []
    for table, columns in sorted(before.items()):
        if table not in after:
            changes.append(f"drop table {table}")
            continue
        for column, (declared, notnull, _) in sorted(columns.items()):
            if column not in after[table]:
                changes.append(f"drop column {table}.{column}")
                continue
            now_declared, now_notnull, _ = after[table][column]
            if now_declared != declared:
                changes.append(f"retype column {table}.{column}")
            if now_notnull and not notnull:
                changes.append(f"require column {table}.{column}")
        for column, (_, notnull, default) in sorted(after[table].items()):
            if column not in columns and notnull and default is None:
                changes.append(f"require new column {table}.{column}")
    return changes


def contractions(panel: Path) -> list[Contraction]:
    shared = migrations(panel / COMMON / "migrations")
    found = []
    for directory in sorted(panel.glob("*/migrations")):
        module = directory.parent.name
        if module == COMMON:
            continue
        files = sorted(
            shared + migrations(directory),
            key=lambda path: int(MIGRATION.fullmatch(path.name).group("version")),
        )
        database = sqlite3.connect(":memory:")
        database.isolation_level = None
        before: dict = {}
        for path in files:
            try:
                database.executescript(path.read_text(encoding="utf-8"))
            except sqlite3.Error as error:
                raise PolicyError(f"{module}: {path.name} does not apply: {error}") from error
            after = schema(database)
            if path.parent == directory:
                found.extend(Contraction(module, path.name, change) for change in compare(before, after))
            before = after
        database.close()
    return found


def records(document: object) -> dict[Contraction, str]:
    if not isinstance(document, dict) or set(document) != {"schema_version", "contractions"}:
        raise PolicyError(f"{POLICY} holds exactly schema_version and contractions")
    if document["schema_version"] != 1:
        raise PolicyError(f"{POLICY}: schema_version 1 is the one this guard reads")
    entries = document["contractions"]
    if not isinstance(entries, list):
        raise PolicyError(f"{POLICY}: contractions is a list")
    recorded: dict[Contraction, str] = {}
    for index, entry in enumerate(entries):
        context = f"{POLICY} contraction {index}"
        if not isinstance(entry, dict) or set(entry) != RECORD_FIELDS:
            raise PolicyError(f"{context} holds exactly {', '.join(sorted(RECORD_FIELDS))}")
        owners.REGISTERED.require(entry["owner"], f"{context} owner")
        fields.text(entry["reason"], f"{context} reason")
        release = fields.text(entry["unused_since"], f"{context} unused_since")
        if not RELEASE.fullmatch(release):
            raise PolicyError(f"{context} unused_since names a release such as 0.9.0")
        key = Contraction(
            fields.text(entry["module"], f"{context} module"),
            fields.text(entry["migration"], f"{context} migration"),
            fields.text(entry["change"], f"{context} change"),
        )
        if key in recorded:
            raise PolicyError(f"{context} repeats {key.change} in {key.migration}")
        recorded[key] = release
    return recorded


def check(repo: Path) -> list[str]:
    found = set(contractions(repo / "panel"))
    recorded = records(json.loads((repo / POLICY).read_text(encoding="utf-8")))
    failures = [
        f"{item.module}/migrations/{item.migration}: {item.change} contracts the schema; "
        f"record the release from which nothing used it in {POLICY}"
        for item in sorted(found - recorded.keys())
    ]
    failures.extend(
        f"{POLICY}: {item.module}/migrations/{item.migration} no longer contracts with {item.change}"
        for item in sorted(recorded.keys() - found)
    )
    return failures


if __name__ == "__main__":
    root = Path(__file__).resolve().parents[2]
    try:
        problems = check(root)
    except PolicyError as error:
        print(error, file=sys.stderr)
        sys.exit(2)
    for problem in problems:
        print(problem, file=sys.stderr)
    if problems:
        sys.exit(1)
    print("Panel migrations only expand schemas, or contract what an earlier release left unused.")
