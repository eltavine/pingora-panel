#!/usr/bin/env python3
"""Contract tests for the leased, exact-fingerprint Gitleaks baseline guard."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

CHECKER = Path(__file__).resolve().with_name("check-gitleaks-baseline.py")
TODAY = "2026-06-01"


def run(*arguments: str, cwd: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        list(arguments), cwd=cwd, check=False, capture_output=True, text=True
    )


def check(repository: Path, baseline: Path, registry: Path) -> int:
    return run(
        sys.executable,
        str(CHECKER),
        str(repository),
        "--baseline",
        str(baseline),
        "--registry",
        str(registry),
        "--today",
        TODAY,
        cwd=repository,
    ).returncode


def lease(fingerprints: list[str], **changes: Any) -> dict[str, Any]:
    return {
        "fingerprints": fingerprints,
        "owner": "pingora-panel-security",
        "reason": "test fixture suppression",
        "expires_on": "2026-12-31",
        **changes,
    }


def document(*leases: dict[str, Any], max_fingerprints: int = 8) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "max_fingerprints": max_fingerprints,
        "leases": list(leases),
    }


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="gitleaks-baseline.") as temporary:
        root = Path(temporary)
        repository = root / "repository"
        repository.mkdir()
        run("git", "init", "-q", cwd=repository)
        run("git", "config", "user.name", "Gitleaks Baseline Test", cwd=repository)
        run("git", "config", "user.email", "gitleaks-baseline@example.invalid", cwd=repository)
        (repository / "directory").mkdir()
        (repository / "fixture.txt").write_text("not a secret\n", encoding="utf-8")
        (repository / "directory/nested.txt").write_text("nested\n", encoding="utf-8")
        run("git", "add", "fixture.txt", "directory/nested.txt", cwd=repository)
        committed = run("git", "commit", "-qm", "test: add fixture", cwd=repository)
        if committed.returncode != 0:
            raise AssertionError(committed.stderr)
        commit = run("git", "rev-parse", "HEAD", cwd=repository).stdout.strip()

        fingerprint = f"{commit}:fixture.txt:generic-api-key:1"
        nested = f"{commit}:directory/nested.txt:private-key:1"
        baseline = repository / ".gitleaksignore"
        registry = root / "registry.json"

        def require(expected: int, scenario: str) -> None:
            actual = check(repository, baseline, registry)
            if actual != expected:
                raise AssertionError(f"{scenario}: expected exit {expected}, got {actual}")

        def write(contents: str, policy: dict[str, Any]) -> None:
            baseline.write_text(contents, encoding="utf-8")
            registry.write_text(json.dumps(policy), encoding="utf-8")

        write(f"# exact fixture\n{fingerprint}\n", document(lease([fingerprint])))
        require(0, "leased exact fingerprint")

        write(
            f"{fingerprint}\n{nested}\n",
            document(lease([fingerprint]), lease([nested], owner="pingora-panel-platform")),
        )
        require(0, "one lease per fingerprint group")

        write(f"{fingerprint}\n{nested}\n", document(lease([fingerprint, nested])))
        require(0, "one lease covering several fingerprints")

        # The two files must describe exactly the same set, in both directions.
        write(f"{fingerprint}\n{nested}\n", document(lease([fingerprint])))
        require(1, "baseline entry without a lease")

        write(f"{fingerprint}\n", document(lease([fingerprint, nested])))
        require(1, "lease without a baseline entry")

        write(f"{fingerprint}\n", document(lease([fingerprint]), max_fingerprints=1))
        require(0, "baseline exactly at its ceiling")

        write(
            f"{fingerprint}\n{nested}\n",
            document(lease([fingerprint, nested]), max_fingerprints=1),
        )
        require(2, "registry exceeding its own ceiling")

        write(f"{fingerprint}\n", document(lease([fingerprint], expires_on="2026-05-31")))
        require(2, "expired lease")

        write(f"{fingerprint}\n", document(lease([fingerprint], expires_on="2026-6-01")))
        require(2, "non-canonical lease expiry")

        anonymous = lease([fingerprint])
        del anonymous["owner"]
        write(f"{fingerprint}\n", document(anonymous))
        require(2, "lease without an accountable owner")

        write(f"{fingerprint}\n", document(lease([fingerprint], reason="   ")))
        require(2, "lease without a reason")

        write(f"{fingerprint}\n", document(lease([fingerprint], unexpected=True)))
        require(2, "unknown lease field")

        write(f"{fingerprint}\n", document(lease([fingerprint, fingerprint])))
        require(2, "lease repeating one fingerprint")

        write(f"{fingerprint}\n", document(lease([])))
        require(2, "lease covering no fingerprint")

        write(f"{fingerprint}\n", document())
        require(2, "registry without any lease")

        unsupported = document(lease([fingerprint]))
        unsupported["schema_version"] = 99
        write(f"{fingerprint}\n", unsupported)
        require(2, "unsupported registry schema")

        oversized = document(lease([fingerprint]))
        oversized["max_fingerprints"] = 0
        write(f"{fingerprint}\n", oversized)
        require(2, "non-positive fingerprint ceiling")

        # Every fingerprint must still resolve to a real line of a real blob.
        unresolvable = {
            "duplicate": f"{fingerprint}\n{fingerprint}\n",
            "partial": "fixture.txt:generic-api-key:1\n",
            "missing-commit": f"{'0' * 40}:fixture.txt:generic-api-key:1\n",
            "missing-path": f"{commit}:missing.txt:generic-api-key:1\n",
            "parent-path": f"{commit}:../fixture.txt:generic-api-key:1\n",
            "noncanonical-path": f"{commit}:directory//nested.txt:generic-api-key:1\n",
            "tree-path": f"{commit}:directory:generic-api-key:1\n",
            "line-out-of-range": f"{commit}:fixture.txt:generic-api-key:2\n",
        }
        for scenario, contents in unresolvable.items():
            entry = contents.strip().splitlines()[0]
            write(contents, document(lease([entry]) if ":" in entry else lease([fingerprint])))
            if check(repository, baseline, registry) == 0:
                raise AssertionError(f"{scenario} baseline was not rejected")

        # A linked worktree must be accepted, since CI checks out that way.
        worktree = root / "linked-worktree"
        added = run("git", "worktree", "add", "--detach", str(worktree), "HEAD", cwd=repository)
        if added.returncode != 0:
            raise AssertionError(added.stderr)
        worktree_baseline = worktree / ".gitleaksignore"
        worktree_baseline.write_text(f"{fingerprint}\n", encoding="utf-8")
        registry.write_text(json.dumps(document(lease([fingerprint]))), encoding="utf-8")
        if check(worktree, worktree_baseline, registry) != 0:
            raise AssertionError("valid linked Git worktree was rejected")

    print("Gitleaks baseline policy self-test passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
