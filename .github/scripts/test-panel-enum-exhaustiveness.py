#!/usr/bin/env python3
"""Self-test for the public enum exhaustiveness guard.

Each scenario builds a throwaway workspace, so the cases keep meaning what they
mean as the real workspace gains and loses enums. The exit codes are the
contract: 0 accepted, 1 a decision is missing or contradicted, 2 the verdict
could not be reached.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Callable

CHECKER = Path(__file__).resolve().parent / "check-panel-enum-exhaustiveness.py"

MANIFEST = """[package]
name = "fixture-crate"
version = "0.1.0"
edition = "2021"
"""

#: One enum of each kind: marked, and recorded as deliberately exhaustive.
SOURCE = """#[non_exhaustive]
pub enum Grows {
    First,
}

pub enum Closed {
    Only,
}
"""


def decision(name: str = "Closed", **overrides: object) -> dict:
    entry = {
        "package": "fixture-crate",
        "name": name,
        "owner": "pingora-panel-platform",
        "reason": "the fixture encoder must fail to compile on a new variant",
    }
    entry.update(overrides)
    return entry


def document(*entries: dict, schema_version: int = 1) -> dict:
    return {"schema_version": schema_version, "exhaustive_enums": list(entries)}


def build(root: Path, source: str = SOURCE, policy: dict | None = None) -> Path:
    crate = root / "panel" / "fixture-crate"
    (crate / "src").mkdir(parents=True)
    (crate / "Cargo.toml").write_text(MANIFEST, encoding="utf-8")
    (crate / "src" / "lib.rs").write_text(source, encoding="utf-8")
    registry = root / "policy.json"
    registry.write_text(
        json.dumps(policy if policy is not None else document(decision()), indent=2),
        encoding="utf-8",
    )
    return registry


def check(root: Path, registry: Path) -> int:
    return subprocess.run(
        [
            sys.executable,
            str(CHECKER),
            str(root / "panel"),
            "--registry",
            str(registry),
        ],
        check=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    ).returncode


def require(
    scenario: str,
    expected: int,
    source: str = SOURCE,
    policy: dict | None = None,
    mutate: Callable[[Path, Path], object] | None = None,
) -> None:
    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        registry = build(root, source, policy)
        if mutate is not None:
            mutate(root, registry)
        actual = check(root, registry)
    if actual != expected:
        raise AssertionError(f"{scenario}: expected exit {expected}, got {actual}")


def main() -> int:
    require("a marked enum and a recorded one", 0)

    # The drift this guard exists to catch: an enum whose ability to grow was
    # decided by whoever typed it and recorded nowhere.
    require(
        "a bare public enum with no decision",
        1,
        source=SOURCE + "\npub enum Undecided {\n    Sole,\n}\n",
    )
    require(
        "a bare public enum and an empty record",
        1,
        policy=document(),
    )

    # A record that disagrees with the code is worse than no record, because it
    # reads as though the question was settled.
    require(
        "a record contradicted by #[non_exhaustive]",
        1,
        policy=document(decision(name="Grows"), decision()),
    )
    require(
        "a record naming an enum that no longer exists",
        1,
        policy=document(decision(), decision(name="Departed")),
    )

    # The attribute counts wherever it sits in the block, not only directly
    # above the declaration.
    require(
        "the attribute below another attribute",
        0,
        source='#[derive(Debug)]\n#[non_exhaustive]\npub enum Grows {\n    First,\n}\n'
        "\npub enum Closed {\n    Only,\n}\n",
    )
    require(
        "the attribute above another attribute",
        0,
        source='#[non_exhaustive]\n#[derive(Debug)]\npub enum Grows {\n    First,\n}\n'
        "\npub enum Closed {\n    Only,\n}\n",
    )
    require(
        "attribute text that only resembles non_exhaustive",
        1,
        source='#[doc = "non_exhaustive"]\npub enum Grows {\n    First,\n}\n'
        "\npub enum Closed {\n    Only,\n}\n",
    )
    require(
        "duplicate public enum identities",
        2,
        source=SOURCE + "\npub mod nested {\n    pub enum Closed {\n        Other,\n    }\n}\n",
    )

    # Test modules are scanned conservatively. Truncating the file at a test
    # module would let a later production declaration evade the guard; test-only
    # types do not need public visibility.
    require(
        "a bare enum inside a test module",
        1,
        source=SOURCE + "\n#[cfg(test)]\nmod tests {\n    pub enum Local {\n        A,\n    }\n}\n",
    )
    require(
        "a production enum declared after a test module",
        1,
        source=SOURCE
        + "\n#[cfg(test)]\nmod tests {}\n\npub enum AfterTests {\n    A,\n}\n",
    )
    # A private enum cannot break a caller, so it is not the subject here.
    require(
        "a private enum",
        0,
        source=SOURCE + "\nenum Hidden {\n    A,\n}\n",
    )

    # Accountability on the record is held to the same rules as everywhere else.
    require(
        "a decision naming nobody accountable",
        2,
        policy=document(decision(owner="nobody-in-particular")),
    )
    for field in ("package", "name", "owner", "reason"):
        require(
            f"a decision missing {field}",
            2,
            policy=document(
                {key: value for key, value in decision().items() if key != field}
            ),
        )
    require(
        "a decision carrying an unknown field",
        2,
        policy=document({**decision(), "expires_on": "2027-01-01"}),
    )
    require(
        "a decision repeated",
        2,
        policy=document(decision(), decision()),
    )
    require(
        "an unsupported schema version",
        2,
        policy=document(decision(), schema_version=2),
    )
    require(
        "a malformed record",
        2,
        mutate=lambda _root, registry: registry.write_text("{", encoding="utf-8"),
    )
    require(
        "a record that is not there",
        2,
        mutate=lambda _root, registry: registry.unlink(),
    )
    require(
        "a workspace with no members",
        2,
        mutate=lambda root, _registry: shutil.rmtree(root / "panel" / "fixture-crate"),
    )
    require(
        "a workspace with no public enums",
        2,
        source="pub struct OnlyAStruct;\n",
    )

    print("Panel enum exhaustiveness self-test passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
