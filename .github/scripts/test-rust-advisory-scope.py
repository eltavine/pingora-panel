#!/usr/bin/env python3
"""Contract tests for the per-workspace advisory exception scope guard.

The applied set for one workspace is every leased advisory that cargo-deny did
not report as unmatched, so these fixtures drive both directions: a claim that
matches nothing, and a suppression that nothing claims.
"""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

CHECKER = Path(__file__).resolve().with_name("check-rust-advisory-scope.py")
UPSTREAM = "Cargo.toml"
PANEL = "panel/Cargo.toml"
TODAY = "2026-06-01"

SUMMARY = {"type": "summary", "fields": {"advisories": {"errors": 0, "warnings": 2}}}


def not_detected(identifier: str) -> dict[str, Any]:
    return {
        "type": "diagnostic",
        "fields": {
            "code": "advisory-not-detected",
            "message": "advisory was not encountered",
            "severity": "warning",
            "graphs": [],
            "labels": [
                {
                    "column": 6,
                    "line": 15,
                    "message": "no crate matched advisory criteria",
                    "span": identifier,
                }
            ],
        },
    }


def exception(identifier: str, workspaces: list[str]) -> dict[str, Any]:
    return {
        "advisory_id": identifier,
        "workspaces": workspaces,
        "expires_on": "2026-12-31",
        "owner": "pingora-panel-security",
        "reason": "test fixture exception",
        "scope": "test-only dependency path",
    }


def registry(*exceptions: dict[str, Any]) -> dict[str, Any]:
    return {"schema_version": 2, "exceptions": list(exceptions)}


UPSTREAM_ONLY = registry(
    exception("RUSTSEC-2026-0001", [UPSTREAM]),
    exception("RUSTSEC-2026-0002", [UPSTREAM]),
)
BOTH_WORKSPACES = registry(
    exception("RUSTSEC-2026-0001", [UPSTREAM, PANEL]),
    exception("RUSTSEC-2026-0002", [UPSTREAM, PANEL]),
)
PANEL_ONLY = registry(
    exception("RUSTSEC-2026-0001", [PANEL]),
    exception("RUSTSEC-2026-0002", [PANEL]),
)


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
    root: Path, workspace: str, report: list[dict[str, Any]], document: dict[str, Any]
) -> int:
    report_path = root / "report.jsonl"
    registry_path = root / "registry.json"
    report_path.write_text(
        "".join(f"{json.dumps(message)}\n" for message in report), encoding="utf-8"
    )
    registry_path.write_text(json.dumps(document), encoding="utf-8")
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


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="rust-advisory-scope.") as temporary:
        root = Path(temporary).resolve()
        build_repository(root)

        def require(
            expected: int,
            scenario: str,
            workspace: str,
            report: list[dict[str, Any]],
            document: dict[str, Any] = UPSTREAM_ONLY,
        ) -> None:
            actual = check(root, workspace, report, document)
            if actual != expected:
                raise AssertionError(f"{scenario}: expected exit {expected}, got {actual}")

        # Upstream applies both exceptions, so nothing is reported unmatched.
        require(0, "every claimed exception applies", UPSTREAM, [SUMMARY])
        # Panel claims neither, and cargo-deny reports both as unmatched there.
        require(
            0,
            "an unclaimed workspace reports every exception unmatched",
            PANEL,
            [not_detected("RUSTSEC-2026-0001"), not_detected("RUSTSEC-2026-0002"), SUMMARY],
        )

        # An unclaimed suppression fails in every workspace: this is the property
        # that keeps the Panel tree from inheriting an upstream ignore.
        require(
            1,
            "a suppression that no exception claims",
            PANEL,
            [not_detected("RUSTSEC-2026-0001"), SUMMARY],
        )
        require(
            1,
            "an unclaimed suppression in the floating workspace",
            UPSTREAM,
            [SUMMARY],
            PANEL_ONLY,
        )

        # A stale claim is only required to fail where resolution stands still.
        require(
            1,
            "a stale claim in the workspace with a committed lockfile",
            PANEL,
            [not_detected("RUSTSEC-2026-0001"), not_detected("RUSTSEC-2026-0002"), SUMMARY],
            BOTH_WORKSPACES,
        )
        require(
            0,
            "resolution churn retires a claim in the floating workspace",
            UPSTREAM,
            [not_detected("RUSTSEC-2026-0001"), SUMMARY],
        )
        require(
            0,
            "resolution churn retires every claim in the floating workspace",
            UPSTREAM,
            [not_detected("RUSTSEC-2026-0001"), not_detected("RUSTSEC-2026-0002"), SUMMARY],
        )

        require(
            0,
            "an exception applying in both claimed workspaces",
            PANEL,
            [SUMMARY],
            BOTH_WORKSPACES,
        )
        require(
            1,
            "an unmatched advisory the registry does not lease",
            UPSTREAM,
            [not_detected("RUSTSEC-2026-9999"), SUMMARY],
        )

        require(
            2,
            "cargo-deny report without a completion summary",
            UPSTREAM,
            [not_detected("RUSTSEC-2026-0001")],
        )
        require(
            2,
            "cargo-deny summary missing the advisories check",
            UPSTREAM,
            [{"type": "summary", "fields": {"bans": {"errors": 0, "warnings": 0}}}],
        )
        require(
            2,
            "unmatched advisory without a single identifying label",
            UPSTREAM,
            [
                {
                    "type": "diagnostic",
                    "fields": {
                        "code": "advisory-not-detected",
                        "severity": "warning",
                        "graphs": [],
                        "labels": [],
                    },
                },
                SUMMARY,
            ],
        )
        require(
            2,
            "unmatched advisory with a malformed label",
            UPSTREAM,
            [
                {
                    "type": "diagnostic",
                    "fields": {
                        "code": "advisory-not-detected",
                        "severity": "warning",
                        "graphs": [],
                        "labels": [{"span": "CVE-2026-0001"}],
                    },
                },
                SUMMARY,
            ],
        )
        require(
            2,
            "workspace argument that is not a Cargo manifest",
            "panel/src/lib.rs",
            [SUMMARY],
        )

    print("Rust advisory scope policy self-test passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
