#!/usr/bin/env python3
"""Contract tests for synchronized Rust advisory exception policies."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

CHECKER = Path(__file__).resolve().with_name("check-rust-advisory-policy.py")
WORKSPACES = ["Cargo.toml"]


def write_config(path: Path, values: list[str]) -> None:
    entries = ", ".join(f'"{value}"' for value in values)
    path.write_text(f"[advisories]\nignore = [{entries}]\n", encoding="utf-8")


def registry_document(
    values: list[str],
    expires_on: str = "2026-12-31",
    workspaces: list[str] | None = None,
) -> dict[str, Any]:
    return {
        "schema_version": 2,
        "exceptions": [
            {
                "advisory_id": value,
                "workspaces": WORKSPACES if workspaces is None else workspaces,
                "expires_on": expires_on,
                "owner": "pingora-panel-security",
                "reason": "test fixture exception",
                "scope": "test-only dependency path",
            }
            for value in values
        ],
    }


def write_registry(path: Path, document: dict[str, Any]) -> None:
    path.write_text(json.dumps(document), encoding="utf-8")


def check_result(
    audit: Path, deny: Path, registry: Path
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            sys.executable,
            str(CHECKER),
            "--audit-config",
            str(audit),
            "--deny-config",
            str(deny),
            "--registry",
            str(registry),
            "--today",
            "2026-01-01",
        ],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )


def check(audit: Path, deny: Path, registry: Path) -> int:
    return check_result(audit, deny, registry).returncode


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="rust-advisory-policy.") as temporary:
        root = Path(temporary)
        audit = root / "audit.toml"
        deny = root / "deny.toml"
        registry = root / "registry.json"
        identifiers = ["RUSTSEC-2026-0001", "RUSTSEC-2026-0002"]

        def require(expected: int, scenario: str) -> None:
            actual = check(audit, deny, registry)
            if actual != expected:
                raise AssertionError(f"{scenario}: expected exit {expected}, got {actual}")

        write_config(audit, identifiers)
        write_config(deny, list(reversed(identifiers)))
        write_registry(registry, registry_document(identifiers))
        require(0, "equivalent advisory policies")

        write_config(deny, identifiers[:1])
        require(1, "divergent advisory policies")
        divergence = check_result(audit, deny, registry)
        if identifiers[1] not in divergence.stderr or "missing-from-deny" not in divergence.stderr:
            raise AssertionError(
                f"divergent policy diagnostics hid the missing entry: {divergence.stderr}"
            )

        write_config(deny, [identifiers[0], identifiers[0]])
        require(2, "duplicate advisory exception in a config")

        write_config(deny, ["CVE-2026-0001"])
        require(2, "malformed advisory identifier")

        write_config(deny, identifiers)
        write_registry(registry, registry_document(identifiers[:1]))
        require(1, "advisory ignored without a registry lease")

        write_registry(registry, registry_document(identifiers, expires_on="2025-12-31"))
        require(2, "expired advisory lease")

        write_registry(registry, registry_document(identifiers, expires_on="2026-1-31"))
        require(2, "non-canonical advisory expiry")

        write_registry(registry, registry_document(identifiers, workspaces=[]))
        require(2, "exception that claims no workspace")

        write_registry(
            registry, registry_document(identifiers, workspaces=["panel/lib.rs"])
        )
        require(2, "workspace claim that is not a Cargo manifest")

        write_registry(
            registry,
            registry_document(identifiers, workspaces=["panel/Cargo.toml", "Cargo.toml"]),
        )
        require(2, "unsorted workspace claims")

        write_registry(
            registry,
            registry_document(identifiers, workspaces=["Cargo.toml", "Cargo.toml"]),
        )
        require(2, "repeated workspace claim")

        document = registry_document(identifiers)
        document["exceptions"][0]["unknown"] = True
        write_registry(registry, document)
        require(2, "unknown advisory lease field")

        document = registry_document(identifiers)
        del document["exceptions"][0]["scope"]
        write_registry(registry, document)
        require(2, "advisory lease without a scope")

        document = registry_document([identifiers[0], identifiers[0]])
        write_registry(registry, document)
        require(2, "duplicate advisory lease in the registry")

        document = registry_document(identifiers)
        document["schema_version"] = 1
        write_registry(registry, document)
        require(2, "unscoped v1 registry is no longer accepted")

        document = registry_document(identifiers)
        document["schema_version"] = 99
        write_registry(registry, document)
        require(2, "unsupported registry schema")

    print("Rust advisory policy self-test passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
