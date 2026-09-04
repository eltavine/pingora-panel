#!/usr/bin/env python3
"""Contract tests for version-pinned workflow runner labels."""

from __future__ import annotations

import subprocess
import sys
import tempfile
from pathlib import Path


CHECKER = Path(__file__).resolve().with_name("check-workflow-runners-pinned.py")


def check(workflow: str) -> int:
    with tempfile.TemporaryDirectory(prefix="workflow-runner-policy.") as root:
        workflow_root = Path(root)
        (workflow_root / "fixture.yml").write_text(workflow, encoding="utf-8")
        return subprocess.run(
            [sys.executable, str(CHECKER), str(workflow_root)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        ).returncode


def expect(workflow: str, expected: int, scenario: str) -> None:
    actual = check(workflow)
    if actual != expected:
        raise AssertionError(f"{scenario}: expected exit code {expected}, got {actual}")


def main() -> int:
    expect("jobs:\n  test:\n    runs-on: ubuntu-24.04\n", 0, "pinned runner")
    expect("# runs-on: ubuntu-latest\njobs: {}\n", 0, "commented label")
    expect("jobs:\n  test:\n    runs-on: ubuntu-latest\n", 1, "floating runner")
    expect(
        "jobs:\n  test:\n    runs-on: ubuntu-latest#not-a-yaml-comment\n",
        1,
        "hash embedded in a scalar",
    )
    expect(
        "jobs:\n  test:\n    runs-on: '${{ matrix.os }}'\n    # ubuntu-latest\n",
        0,
        "expression with commented label",
    )
    expect(
        "strategy:\n  matrix:\n    os: [ubuntu-24.04, windows-latest]\n",
        1,
        "floating matrix runner",
    )
    with tempfile.TemporaryDirectory(prefix="workflow-runner-policy-empty.") as root:
        if (
            subprocess.run(
                [sys.executable, str(CHECKER), root],
                check=False,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            ).returncode
            == 0
        ):
            raise AssertionError("empty workflow directory was not rejected")

    print("Workflow runner label guard self-test passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
