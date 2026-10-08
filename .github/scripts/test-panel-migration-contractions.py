#!/usr/bin/env python3
"""Negative and positive contract cases for the migration contraction guard."""

from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
guard = Path(__file__).with_name("check-panel-migration-contractions.py")
spec = importlib.util.spec_from_file_location("migration_contraction_guard", guard)
assert spec is not None and spec.loader is not None
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

from policy import PolicyError  # noqa: E402

SHARED = "CREATE TABLE outbox (position INTEGER PRIMARY KEY, event TEXT NOT NULL);"
FIRST = "CREATE TABLE sites (id TEXT PRIMARY KEY, name TEXT NOT NULL, note TEXT);"


def repository(migrations: dict[str, str], recorded: list[dict] | None = None) -> Path:
    root = Path(tempfile.mkdtemp())
    (root / "panel/panel-sqlite/migrations").mkdir(parents=True)
    (root / "panel/panel-sqlite/migrations/0001_outbox.sql").write_text(SHARED)
    (root / "panel/sites-service/migrations").mkdir(parents=True)
    for name, sql in migrations.items():
        (root / "panel/sites-service/migrations" / name).write_text(sql)
    (root / ".github/policies").mkdir(parents=True)
    (root / module.POLICY).write_text(
        json.dumps({"schema_version": 1, "contractions": recorded or []})
    )
    return root


def record(change: str, migration: str = "10001_next.sql", **extra) -> dict:
    entry = {
        "module": "sites-service",
        "migration": migration,
        "change": change,
        "unused_since": "0.9.0",
        "owner": "pingora-panel-platform",
        "reason": "0.9.0 stopped reading the note.",
    }
    entry.update(extra)
    return entry


def failures(migrations: dict[str, str], recorded: list[dict] | None = None) -> list[str]:
    return module.check(repository({"10000_sites.sql": FIRST, **migrations}, recorded))


def fails(found: list[str], fragment: str) -> None:
    assert any(fragment in failure for failure in found), (fragment, found)


# Expanding passes: a table, a nullable column, a NOT NULL column with a default.
assert failures({
    "10001_next.sql": "CREATE TABLE routes (id TEXT PRIMARY KEY);"
    "ALTER TABLE sites ADD COLUMN tag TEXT;"
    "ALTER TABLE sites ADD COLUMN weight INTEGER NOT NULL DEFAULT 1;"
}) == []

# A table rebuilt with all its columns passes, wrapped as the runtime applies it.
REBUILD = """PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;
CREATE TABLE sites_next (id TEXT PRIMARY KEY, name TEXT NOT NULL, note TEXT,
    kind TEXT CHECK (kind IN ('proxy', 'static')));
INSERT INTO sites_next (id, name, note) SELECT id, name, note FROM sites;
DROP TABLE sites;
ALTER TABLE sites_next RENAME TO sites;
COMMIT;
PRAGMA foreign_keys = ON;
"""
assert failures({"10001_next.sql": REBUILD}) == []

DROP = "ALTER TABLE sites DROP COLUMN note;"
fails(failures({"10001_next.sql": DROP}), "drop column sites.note contracts the schema")
assert failures({"10001_next.sql": DROP}, [record("drop column sites.note")]) == []
fails(
    failures({"10001_next.sql": REBUILD}, [record("drop column sites.note")]),
    "no longer contracts with drop column sites.note",
)

RETYPE = REBUILD.replace("note TEXT,", "note BLOB,")
fails(failures({"10001_next.sql": RETYPE}), "retype column sites.note")
REQUIRE = REBUILD.replace("note TEXT,", "note TEXT NOT NULL DEFAULT '',")
fails(failures({"10001_next.sql": REQUIRE}), "require column sites.note")
NEW = REBUILD.replace("note TEXT,", "note TEXT, owner TEXT NOT NULL,").replace(
    "SELECT id, name, note", "SELECT id, name, note"
)
fails(failures({"10001_next.sql": NEW}), "require new column sites.owner")
fails(failures({"10001_next.sql": "DROP TABLE sites;"}), "drop table sites")

# Records are held to an owner, a reason and a release.
for bad in (
    record("drop column sites.note", owner="nobody"),
    record("drop column sites.note", unused_since="next"),
    {key: value for key, value in record("drop column sites.note").items() if key != "reason"},
):
    try:
        failures({"10001_next.sql": DROP}, [bad])
    except PolicyError:
        pass
    else:
        raise AssertionError(f"accepted {bad}")

# A migration that does not apply is reported, not skipped.
try:
    failures({"10001_next.sql": "ALTER TABLE nowhere ADD COLUMN x TEXT;"})
except PolicyError as error:
    assert "does not apply" in str(error), error
else:
    raise AssertionError("a broken migration passed")

print("Panel migration contraction guard self-test passed.")
