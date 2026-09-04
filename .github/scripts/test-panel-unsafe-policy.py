#!/usr/bin/env python3
"""Contract tests for compiler-enforced Panel unsafe isolation.

The exemption from `#![forbid(unsafe_code)]` is granted by a lease rather than
by the guard, so these cover both halves: a crate cannot hold unsafe without a
live lease, and a lease cannot outlive the crate it was written for.
"""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path

CHECKER = Path(__file__).resolve().with_name("check-panel-unsafe-policy.py")

SAFE = "#![forbid(unsafe_code)]\n"
ADAPTER = (
    "#![forbid(clippy::undocumented_unsafe_blocks)]\n"
    "#![forbid(unsafe_op_in_unsafe_fn)]\n"
)


def create_fixture(root: Path, safe_root: str, adapter_root: str) -> Path:
    (root / "safe-core" / "src").mkdir(parents=True)
    (root / "snapshot-store-fs" / "src").mkdir(parents=True)
    (root / "Cargo.toml").write_text(
        '[workspace]\nresolver = "2"\nmembers = ["safe-core", "snapshot-store-fs"]\n',
        encoding="utf-8",
    )
    for package in ("safe-core", "snapshot-store-fs"):
        (root / package / "Cargo.toml").write_text(
            f'[package]\nname = "{package}"\nversion = "0.1.0"\nedition = "2021"\n',
            encoding="utf-8",
        )
    (root / "safe-core" / "src" / "lib.rs").write_text(safe_root, encoding="utf-8")
    (root / "snapshot-store-fs" / "src" / "lib.rs").write_text(
        adapter_root, encoding="utf-8"
    )
    subprocess.run(
        ["cargo", "generate-lockfile", "--manifest-path", str(root / "Cargo.toml"), "--offline"],
        check=True,
        stdout=subprocess.DEVNULL,
    )
    return root / "Cargo.toml"


def write_registry(path: Path, adapters: list[dict], max_adapters: int = 2) -> Path:
    path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "max_adapters": max_adapters,
                "adapters": adapters,
            }
        ),
        encoding="utf-8",
    )
    return path


#: The date every scenario is evaluated at, so a fixture cannot pass today and
#: fail next year. Dodging expiry with a far-future date is itself refused now.
TODAY = "2026-09-04"
LIVE = "2027-01-01"


def lease(package: str, expires_on: str = LIVE) -> dict:
    return {
        "package": package,
        "owner": "pingora-panel-platform",
        "reason": "fixture lease",
        "expires_on": expires_on,
    }


def check(manifest: Path, registry: Path) -> int:
    return subprocess.run(
        [
            sys.executable,
            str(CHECKER),
            str(manifest),
            "--registry",
            str(registry),
            "--today",
            TODAY,
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    ).returncode


def expect(check_result: int, expected: int, scenario: str) -> None:
    if check_result != expected:
        raise AssertionError(
            f"{scenario}: expected exit code {expected}, got {check_result}"
        )


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="panel-unsafe-policy.") as temporary:
        root = Path(temporary)
        leased = write_registry(root / "leased.json", [lease("snapshot-store-fs")])

        def scenario(name: str, safe: str, adapter: str, registry: Path) -> int:
            return check(create_fixture(root / name, safe, adapter), registry)

        expect(scenario("valid", SAFE, ADAPTER, leased), 0, "valid policy")
        expect(
            scenario("missing-safe", "", ADAPTER, leased),
            1,
            "missing safe-core attribute",
        )
        expect(
            scenario("missing-adapter-lint", SAFE, "", leased),
            1,
            "missing unsafe-adapter attributes",
        )
        expect(
            scenario("commented-safe", f"/*\n{SAFE}*/\n", ADAPTER, leased),
            1,
            "commented-out safe-core attribute",
        )
        expect(
            scenario("commented-adapter", SAFE, f"/*\n{ADAPTER}*/\n", leased),
            1,
            "commented-out unsafe-adapter attributes",
        )

        # Holding unsafe without a lease is the case the registry exists for: the
        # crate satisfies the adapter attributes but nobody signed for it, so it
        # is held to forbidding unsafe like every other crate.
        unleased = write_registry(root / "unleased.json", [lease("safe-core")])
        expect(
            scenario("unleased-adapter", SAFE, ADAPTER, unleased),
            1,
            "unsafe adapter with no lease",
        )

        # A lease outliving its crate would pre-authorise unsafe in whatever
        # crate later takes that name.
        stale = write_registry(
            root / "stale.json", [lease("snapshot-store-fs"), lease("departed-crate")]
        )
        expect(scenario("stale-lease", SAFE, ADAPTER, stale), 1, "stale adapter lease")

        expired = write_registry(
            root / "expired.json", [lease("snapshot-store-fs", "2020-01-01")]
        )
        expect(scenario("expired-lease", SAFE, ADAPTER, expired), 2, "expired lease")

        over = write_registry(
            root / "over.json",
            [lease("snapshot-store-fs"), lease("safe-core")],
            max_adapters=1,
        )
        expect(scenario("over-ceiling", SAFE, ADAPTER, over), 2, "leases over ceiling")

        duplicated = write_registry(
            root / "duplicated.json",
            [lease("snapshot-store-fs"), lease("snapshot-store-fs")],
        )
        expect(
            scenario("duplicate-lease", SAFE, ADAPTER, duplicated),
            2,
            "the same crate leased twice",
        )

        unaccountable = root / "unaccountable.json"
        unaccountable.write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "max_adapters": 2,
                    "adapters": [{"package": "snapshot-store-fs"}],
                }
            ),
            encoding="utf-8",
        )
        expect(
            scenario("unaccountable", SAFE, ADAPTER, unaccountable),
            2,
            "a lease with no owner, reason, or expiry",
        )

        unsupported = root / "unsupported.json"
        unsupported.write_text(
            json.dumps({"schema_version": 99, "max_adapters": 2, "adapters": []}),
            encoding="utf-8",
        )
        expect(
            scenario("unsupported-schema", SAFE, ADAPTER, unsupported),
            2,
            "an unsupported registry schema",
        )

    print("Panel unsafe isolation policy self-test passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
