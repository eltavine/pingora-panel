#!/usr/bin/env python3
"""Keep security-policy dependency resolution on one approved Rust release."""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path


JOB = re.compile(r"^  ([A-Za-z0-9_-]+):\s*$", re.MULTILINE)
RUST_VERSION = re.compile(r'^\s+rust-version:\s*["\']?([^"\'\s#]+)', re.MULTILINE)
TOOLCHAIN = re.compile(r'^\s+toolchain:\s*["\']?([^"\'\s#]+)', re.MULTILINE)


def job_block(workflow: str, name: str) -> str:
    matches = list(JOB.finditer(workflow))
    for index, match in enumerate(matches):
        if match.group(1) == name:
            end = matches[index + 1].start() if index + 1 < len(matches) else len(workflow)
            return workflow[match.start() : end]
    raise ValueError(f"workflow has no {name!r} job")


def one_value(pattern: re.Pattern[str], block: str, description: str) -> str:
    values = pattern.findall(block)
    if len(values) != 1:
        raise ValueError(f"{description} must appear exactly once, found {len(values)}")
    return values[0]


def verify(workflow: Path, policy: Path) -> str:
    text = workflow.read_text(encoding="utf-8")
    cargo_deny = one_value(
        RUST_VERSION,
        job_block(text, "dependency-policy"),
        "dependency-policy rust-version",
    )
    shared_report = one_value(
        TOOLCHAIN,
        job_block(text, "dependency-leases"),
        "dependency-leases toolchain",
    )
    if cargo_deny != shared_report:
        raise ValueError(
            "security resolver drift: cargo-deny action uses "
            f"{cargo_deny}, shared report uses {shared_report}"
        )

    document = json.loads(policy.read_text(encoding="utf-8"))
    approved = document.get("exact_versions")
    if not isinstance(approved, list) or cargo_deny not in approved:
        raise ValueError(f"security resolver {cargo_deny} is not an approved exact toolchain")
    return cargo_deny


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--workflow", type=Path, default=Path(".github/workflows/audit.yml")
    )
    parser.add_argument(
        "--policy", type=Path, default=Path(".github/policies/rust-toolchains.json")
    )
    args = parser.parse_args()
    try:
        version = verify(args.workflow, args.policy)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"Security resolver contract failed: {error}", file=sys.stderr)
        return 1
    print(f"Security resolver contract verified at Rust {version}.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
