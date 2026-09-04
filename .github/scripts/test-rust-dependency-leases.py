#!/usr/bin/env python3
"""Contract tests for the cargo-deny finding accountability guard.

Covers both enforcement modes: exact identity leases for a workspace with a
committed lockfile, and leased count ceilings for a workspace whose lockfile is
untracked and therefore re-resolves on every run.
"""

from __future__ import annotations

import copy
import json
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

CHECKER = Path(__file__).resolve().with_name("check-rust-dependency-leases.py")
PINNED = "panel/Cargo.toml"
FLOATING = "Cargo.toml"
TODAY = "2026-06-01"

SUMMARY = {
    "type": "summary",
    "fields": {"advisories": {"errors": 0, "warnings": 1}, "bans": {"errors": 0, "warnings": 1}},
}


def graphs(name: str, versions: list[str]) -> list[dict[str, Any]]:
    return [{"Krate": {"name": name, "version": version}} for version in versions]


def duplicate_finding(name: str, versions: list[str]) -> dict[str, Any]:
    return {
        "type": "diagnostic",
        "fields": {
            "code": "duplicate",
            "message": f"found {len(versions)} duplicate entries for crate '{name}'",
            "severity": "warning",
            "graphs": graphs(name, versions),
        },
    }


def unmaintained_finding(identifier: str, name: str, version: str) -> dict[str, Any]:
    return {
        "type": "diagnostic",
        "fields": {
            "code": "unmaintained",
            "message": f"{name} is unmaintained",
            "severity": "warning",
            "advisory": {"id": identifier, "package": name},
            "graphs": graphs(name, [version]),
        },
    }


def yanked_finding(name: str, version: str) -> dict[str, Any]:
    return {
        "type": "diagnostic",
        "fields": {
            "code": "yanked",
            "message": f"detected yanked crate {name}",
            "severity": "error",
            "graphs": graphs(name, [version]),
        },
    }


def accountable(**subject: Any) -> dict[str, Any]:
    return {
        **subject,
        "owner": "pingora-panel-platform",
        "reason": "test fixture finding",
        "expires_on": "2026-12-31",
    }


def duplicate_lease(name: str, versions: list[str]) -> dict[str, Any]:
    return accountable(crate=name, versions=versions)


def unmaintained_lease(identifier: str, name: str, versions: list[str]) -> dict[str, Any]:
    return accountable(advisory_id=identifier, crate=name, versions=versions)


def yanked_lease(name: str, versions: list[str]) -> dict[str, Any]:
    return accountable(crate=name, versions=versions)


def ceiling(maximum: int) -> dict[str, Any]:
    return accountable(max_findings=maximum)


REPORT = [
    duplicate_finding("syn", ["1.0.109", "2.0.117"]),
    unmaintained_finding("RUSTSEC-2024-0388", "derivative", "2.2.0"),
    yanked_finding("chacha20", "0.10.1"),
    SUMMARY,
]

IDENTITY_REGISTRY: dict[str, Any] = {
    "schema_version": 1,
    "workspaces": {
        PINNED: {
            "enforcement": "identity",
            "duplicates": [duplicate_lease("syn", ["1.0.109", "2.0.117"])],
            "unmaintained": [
                unmaintained_lease("RUSTSEC-2024-0388", "derivative", ["2.2.0"])
            ],
            "yanked": [yanked_lease("chacha20", ["0.10.1"])],
        }
    },
}

CEILING_REGISTRY: dict[str, Any] = {
    "schema_version": 1,
    "workspaces": {
        FLOATING: {
            "enforcement": "ceilings",
            "ceilings": {
                "duplicates": ceiling(4),
                "unmaintained": ceiling(2),
                "yanked": ceiling(1),
            },
        }
    },
}


def build_repository(root: Path) -> None:
    """Mirror the repository's reproducibility: panel locked, upstream floating.

    The guards read this fact from Git, so the fixture has to be a real
    repository with `panel/Cargo.lock` tracked and the root lockfile untracked.
    """

    def git(*arguments: str) -> None:
        completed = subprocess.run(
            ["git", "-C", str(root), *arguments], check=False, capture_output=True, text=True
        )
        if completed.returncode != 0:
            raise AssertionError(f"git {' '.join(arguments)}: {completed.stderr}")

    (root / "panel").mkdir(parents=True, exist_ok=True)
    (root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
    (root / "Cargo.lock").write_text("version = 4\n", encoding="utf-8")
    (root / "panel/Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
    (root / "panel/Cargo.lock").write_text("version = 4\n", encoding="utf-8")
    (root / ".gitignore").write_text("Cargo.lock\n!panel/Cargo.lock\n", encoding="utf-8")
    git("init", "-q")
    git("config", "user.name", "Policy Fixture")
    git("config", "user.email", "policy-fixture@example.invalid")
    git("add", ".gitignore", "Cargo.toml", "panel/Cargo.toml", "panel/Cargo.lock")
    git("commit", "-qm", "test: fixture workspaces")

def check(
    root: Path, workspace: str, report: list[dict[str, Any]], registry: dict[str, Any]
) -> int:
    report_path = root / "report.jsonl"
    registry_path = root / "registry.json"
    report_path.write_text(
        "".join(f"{json.dumps(message)}\n" for message in report), encoding="utf-8"
    )
    registry_path.write_text(json.dumps(registry), encoding="utf-8")
    return subprocess.run(
        [
            sys.executable,
            str(CHECKER),
            workspace,
            "--registry",
            str(registry_path),
            "--report",
            str(report_path),
            "--repo-root",
            str(root),
            "--today",
            TODAY,
        ],
        check=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    ).returncode


def identity_registry(**changes: Any) -> dict[str, Any]:
    registry = copy.deepcopy(IDENTITY_REGISTRY)
    registry["workspaces"][PINNED].update(changes)
    return registry


def ceiling_registry(**ceilings: Any) -> dict[str, Any]:
    registry = copy.deepcopy(CEILING_REGISTRY)
    registry["workspaces"][FLOATING]["ceilings"].update(ceilings)
    return registry


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="rust-dependency-leases.") as temporary:
        root = Path(temporary).resolve()
        build_repository(root)

        def require(
            expected: int,
            scenario: str,
            report: Any,
            registry: Any,
            workspace: str = PINNED,
        ) -> None:
            actual = check(root, workspace, report, registry)
            if actual != expected:
                raise AssertionError(f"{scenario}: expected exit {expected}, got {actual}")

        # Identity enforcement: an exact lease per finding, in both directions.
        require(0, "every finding holds an identity lease", REPORT, IDENTITY_REGISTRY)
        require(
            1,
            "new duplicate without a lease",
            [*REPORT, duplicate_finding("hashbrown", ["0.15.5", "0.17.1"])],
            IDENTITY_REGISTRY,
        )
        require(
            1,
            "new unmaintained advisory without a lease",
            [*REPORT, unmaintained_finding("RUSTSEC-2024-0436", "paste", "1.0.15")],
            IDENTITY_REGISTRY,
        )
        require(
            1,
            "new yanked crate without a lease",
            [*REPORT, yanked_finding("time", "0.3.36")],
            IDENTITY_REGISTRY,
        )
        require(
            1,
            "duplicate version drift inside a lease",
            [duplicate_finding("syn", ["1.0.109", "2.0.117", "3.0.0"]), *REPORT[1:]],
            IDENTITY_REGISTRY,
        )
        require(
            1,
            "unmaintained version drift inside a lease",
            [
                REPORT[0],
                unmaintained_finding("RUSTSEC-2024-0388", "derivative", "2.3.0"),
                *REPORT[2:],
            ],
            IDENTITY_REGISTRY,
        )
        require(
            1,
            "yanked version drift inside a lease",
            [*REPORT[:2], yanked_finding("chacha20", "0.10.2"), SUMMARY],
            IDENTITY_REGISTRY,
        )
        require(1, "stale duplicate lease", REPORT[1:], IDENTITY_REGISTRY)
        require(1, "stale yanked lease", [*REPORT[:2], SUMMARY], IDENTITY_REGISTRY)

        # Ceiling enforcement: counts, not identities.
        require(0, "counts within every ceiling", REPORT, CEILING_REGISTRY, FLOATING)
        require(
            0,
            "an entirely different crate set within the same counts",
            [
                duplicate_finding("gimli", ["0.29.0", "0.32.3"]),
                unmaintained_finding("RUSTSEC-2025-0134", "rustls-pemfile", "1.0.4"),
                yanked_finding("time", "0.3.36"),
                SUMMARY,
            ],
            CEILING_REGISTRY,
            FLOATING,
        )
        require(
            0,
            "resolution churn that removes findings entirely",
            [duplicate_finding("syn", ["1.0.109", "2.0.117"]), SUMMARY],
            CEILING_REGISTRY,
            FLOATING,
        )
        require(
            1,
            "duplicate count above its ceiling",
            [
                *[
                    duplicate_finding(name, ["1.0.0", "2.0.0"])
                    for name in ("syn", "gimli", "hmac", "sha2", "digest")
                ],
                SUMMARY,
            ],
            CEILING_REGISTRY,
            FLOATING,
        )
        require(
            1,
            "a second yanked crate above its ceiling",
            [
                yanked_finding("chacha20", "0.10.1"),
                yanked_finding("time", "0.3.36"),
                SUMMARY,
            ],
            CEILING_REGISTRY,
            FLOATING,
        )
        require(
            1,
            "a finding kind with no declared ceiling",
            REPORT,
            {
                "schema_version": 1,
                "workspaces": {
                    FLOATING: {
                        "enforcement": "ceilings",
                        "ceilings": {"duplicates": ceiling(4)},
                    }
                },
            },
            FLOATING,
        )
        # A ceiling is reviewed on its expiry, never by comparing it against the
        # live count, because that count is the quantity ceilings exist to stop
        # depending on. A generous ceiling therefore passes until it expires.
        require(
            0,
            "a generous ceiling passes and is governed by its expiry",
            REPORT,
            ceiling_registry(duplicates=ceiling(40)),
            FLOATING,
        )
        require(
            2,
            "a ceiling without an accountable owner",
            REPORT,
            ceiling_registry(
                duplicates={
                    key: value for key, value in ceiling(4).items() if key != "owner"
                }
            ),
            FLOATING,
        )
        require(
            2,
            "an expired ceiling",
            REPORT,
            ceiling_registry(duplicates={**ceiling(4), "expires_on": "2026-05-31"}),
            FLOATING,
        )
        require(
            2,
            "a negative ceiling",
            REPORT,
            ceiling_registry(duplicates=ceiling(-1)),
            FLOATING,
        )
        require(
            2,
            "a non-integer ceiling",
            REPORT,
            ceiling_registry(duplicates={**ceiling(4), "max_findings": "many"}),
            FLOATING,
        )

        # Modes may only use the sections that belong to them.
        require(
            2,
            "identity leases under ceiling enforcement",
            REPORT,
            {
                "schema_version": 1,
                "workspaces": {
                    FLOATING: {
                        "enforcement": "ceilings",
                        "ceilings": {
                            "duplicates": ceiling(4),
                            "unmaintained": ceiling(2),
                            "yanked": ceiling(1),
                        },
                        "duplicates": [duplicate_lease("syn", ["1.0.109", "2.0.117"])],
                    }
                },
            },
            FLOATING,
        )
        require(
            2,
            "ceilings under identity enforcement",
            REPORT,
            identity_registry(ceilings={"duplicates": ceiling(4)}),
        )
        require(
            2,
            "an unknown ceiling section",
            REPORT,
            ceiling_registry(licenses=ceiling(1)),
            FLOATING,
        )
        require(2, "an unknown enforcement mode", REPORT, identity_registry(enforcement="hope"))

        # The declared mode must match the reproducibility Git actually reports,
        # so a registry cannot claim a strictness the repository cannot support.
        require(
            2,
            "identity enforcement claimed for an untracked lockfile",
            REPORT,
            {
                "schema_version": 1,
                "workspaces": {
                    FLOATING: {
                        "enforcement": "identity",
                        "duplicates": [duplicate_lease("syn", ["1.0.109", "2.0.117"])],
                    }
                },
            },
            FLOATING,
        )
        require(
            2,
            "ceiling enforcement settled for a committed lockfile",
            REPORT,
            {
                "schema_version": 1,
                "workspaces": {
                    PINNED: {
                        "enforcement": "ceilings",
                        "ceilings": {
                            "duplicates": ceiling(4),
                            "unmaintained": ceiling(2),
                            "yanked": ceiling(1),
                        },
                    }
                },
            },
        )

        undeclared = copy.deepcopy(IDENTITY_REGISTRY)
        del undeclared["workspaces"][PINNED]["enforcement"]
        require(2, "a workspace without a declared enforcement", REPORT, undeclared)

        # An omitted identity section leaves its findings unaccounted for, so a
        # newly registered finding kind never invalidates an existing document.
        without_yanked = copy.deepcopy(IDENTITY_REGISTRY)
        del without_yanked["workspaces"][PINNED]["yanked"]
        require(1, "omitted identity section leaves findings unleased", REPORT, without_yanked)
        require(
            0,
            "omitted identity section is fine when nothing reports it",
            [*REPORT[:2], SUMMARY],
            without_yanked,
        )

        # Accountability contract for identity leases.
        require(
            2,
            "expired identity lease",
            REPORT,
            identity_registry(
                duplicates=[
                    {
                        **duplicate_lease("syn", ["1.0.109", "2.0.117"]),
                        "expires_on": "2026-05-31",
                    }
                ]
            ),
        )
        require(
            2,
            "identity lease without an accountable owner",
            REPORT,
            identity_registry(
                duplicates=[
                    {
                        key: value
                        for key, value in duplicate_lease(
                            "syn", ["1.0.109", "2.0.117"]
                        ).items()
                        if key != "owner"
                    }
                ]
            ),
        )
        require(
            2,
            "identity lease without a reason",
            REPORT,
            identity_registry(
                duplicates=[
                    {**duplicate_lease("syn", ["1.0.109", "2.0.117"]), "reason": "  "}
                ]
            ),
        )
        require(
            2,
            "identity lease with a non-canonical expiry",
            REPORT,
            identity_registry(
                duplicates=[
                    {
                        **duplicate_lease("syn", ["1.0.109", "2.0.117"]),
                        "expires_on": "2026-6-31",
                    }
                ]
            ),
        )
        require(
            2,
            "identity lease with non-canonical version order",
            REPORT,
            identity_registry(duplicates=[duplicate_lease("syn", ["2.0.117", "1.0.109"])]),
        )
        require(
            2,
            "duplicate lease naming one version",
            REPORT,
            identity_registry(duplicates=[duplicate_lease("syn", ["1.0.109"])]),
        )
        require(
            2,
            "the same identity lease written twice",
            REPORT,
            identity_registry(
                duplicates=[
                    duplicate_lease("syn", ["1.0.109", "2.0.117"]),
                    duplicate_lease("syn", ["1.0.109", "2.0.117"]),
                ]
            ),
        )

        # Document integrity.
        unsupported = copy.deepcopy(IDENTITY_REGISTRY)
        unsupported["schema_version"] = 99
        require(2, "unsupported registry schema", REPORT, unsupported)

        unversioned = copy.deepcopy(IDENTITY_REGISTRY)
        del unversioned["schema_version"]
        require(2, "registry without a schema version", REPORT, unversioned)

        unclassified = copy.deepcopy(IDENTITY_REGISTRY)
        unclassified["workspaces"] = {
            "other/Cargo.toml": unclassified["workspaces"][PINNED]
        }
        require(2, "unclassified workspace", REPORT, unclassified)

        # Report trustworthiness.
        require(2, "report without a completion summary", REPORT[:-1], IDENTITY_REGISTRY)
        require(
            2,
            "summary missing a required check",
            [*REPORT[:-1], {"type": "summary", "fields": {"bans": {"errors": 0}}}],
            IDENTITY_REGISTRY,
        )
        require(
            2,
            "finding reported without a crate graph",
            [
                {
                    "type": "diagnostic",
                    "fields": {"code": "duplicate", "graphs": [], "severity": "warning"},
                },
                SUMMARY,
            ],
            IDENTITY_REGISTRY,
        )
        require(
            2,
            "finding grouped across different crates",
            [
                {
                    "type": "diagnostic",
                    "fields": {
                        "code": "duplicate",
                        "severity": "warning",
                        "graphs": [
                            {"Krate": {"name": "syn", "version": "1.0.109"}},
                            {"Krate": {"name": "quote", "version": "1.0.40"}},
                        ],
                    },
                },
                SUMMARY,
            ],
            IDENTITY_REGISTRY,
        )
        require(
            2,
            "unmaintained finding without an advisory identifier",
            [
                {
                    "type": "diagnostic",
                    "fields": {
                        "code": "unmaintained",
                        "severity": "warning",
                        "graphs": graphs("derivative", ["2.2.0"]),
                    },
                },
                SUMMARY,
            ],
            IDENTITY_REGISTRY,
        )

    print("Rust dependency lease policy self-test passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
