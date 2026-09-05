#!/usr/bin/env python3
"""Contract tests for the security resolver consistency guard."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path


CHECKER = Path(__file__).resolve().with_name("check-security-resolver-contract.py")


def workflow(policy_version: str, report_version: str) -> str:
    return f"""jobs:
  dependency-policy:
    steps:
      - uses: EmbarkStudios/cargo-deny-action@sha
        with:
          rust-version: \"{policy_version}\"
  dependency-leases:
    steps:
      - uses: dtolnay/rust-toolchain@sha
        with:
          toolchain: {report_version}
"""


def run(root: Path, text: str, versions: list[str]) -> int:
    workflow_path = root / "audit.yml"
    policy_path = root / "policy.json"
    workflow_path.write_text(text, encoding="utf-8")
    policy_path.write_text(json.dumps({"exact_versions": versions}), encoding="utf-8")
    return subprocess.run(
        [
            sys.executable,
            str(CHECKER),
            "--workflow",
            str(workflow_path),
            "--policy",
            str(policy_path),
        ],
        check=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    ).returncode


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="security-resolver-contract.") as temporary:
        root = Path(temporary)
        if run(root, workflow("1.98.0", "1.98.0"), ["1.88.0", "1.98.0"]) != 0:
            raise AssertionError("matching approved resolvers were rejected")
        if run(root, workflow("1.88.0", "1.98.0"), ["1.88.0", "1.98.0"]) == 0:
            raise AssertionError("resolver drift was accepted")
        if run(root, workflow("1.99.0", "1.99.0"), ["1.88.0", "1.98.0"]) == 0:
            raise AssertionError("unapproved exact resolver was accepted")
        if run(root, "jobs:\n  dependency-policy:\n", ["1.98.0"]) == 0:
            raise AssertionError("missing dependency-policy resolver was accepted")
    print("Security resolver contract self-test passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
