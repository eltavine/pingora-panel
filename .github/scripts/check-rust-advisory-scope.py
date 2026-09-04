#!/usr/bin/env python3
"""Prove every advisory exception applies exactly where it claims to.

The cargo-audit and cargo-deny ignore lists are shared by every workspace, so an
exception written for the vendored upstream tree silently suppresses advisories
in the Panel tree too. cargo-deny reports an unused ignore as the non-blocking
`advisory-not-detected` warning, which nothing fails on today.

For one workspace, the applied set is every leased advisory that cargo-deny did
*not* report as unmatched. Two properties follow from it:

* Every applied exception must be claimed. This is the property that stops the
  Panel tree from inheriting an ignore written for the vendored upstream tree,
  and it holds regardless of how the workspace resolves.
* Every claim must be applied. A claim that matches nothing is stale, but the
  set of matching advisories only stands still when the lockfile is committed.
  This half is therefore required only of a reproducible workspace, so an
  untracked lockfile re-resolving does not produce failures carrying no
  information.
"""

from __future__ import annotations

import argparse
import sys
from datetime import datetime, timezone
from pathlib import Path

from policy import PolicyError, advisories, cargo_deny, fields, workspaces
from policy.cargo_deny import FindingKindRegistry

CHECKS = ("advisories",)
KINDS = FindingKindRegistry(cargo_deny.ADVISORY_NOT_DETECTED)


def main(argv: list[str] | None = None) -> int:
    repo_root = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("workspace", help="repository-relative workspace manifest path")
    parser.add_argument(
        "--registry",
        type=Path,
        default=repo_root / ".github/policies/rust-advisory-exceptions.json",
    )
    parser.add_argument(
        "--report",
        type=Path,
        help="pre-captured cargo-deny JSON report; cargo-deny runs when omitted",
    )
    parser.add_argument("--repo-root", type=Path, default=repo_root)
    parser.add_argument(
        "--today",
        default=datetime.now(timezone.utc).date().isoformat(),
        help="UTC policy evaluation date in YYYY-MM-DD form",
    )
    arguments = parser.parse_args(argv)

    try:
        today = fields.iso_date(arguments.today, "--today")
        workspace = fields.manifest_path(arguments.workspace, "workspace argument")
        exceptions = advisories.load(arguments.registry, today)
        root = arguments.repo_root.resolve(strict=True)
        reproducible = workspaces.resolves_reproducibly(root, workspace)
        lines = (
            arguments.report.read_text(encoding="utf-8").splitlines()
            if arguments.report is not None
            else cargo_deny.run(root / workspace, root, CHECKS)
        )
        unmatched = {
            finding.advisory_id
            for finding in cargo_deny.parse_report(lines, KINDS, CHECKS)
            if finding.advisory_id is not None
        }
    except (OSError, UnicodeError, PolicyError) as error:
        print(f"Rust advisory scope policy failed closed: {error}", file=sys.stderr)
        return 2

    leased = {exception.advisory_id for exception in exceptions}
    unknown = unmatched - leased
    claimed = {
        exception.advisory_id
        for exception in exceptions
        if exception.claims(workspace)
    }
    applied = leased - unmatched

    failures = [
        f"cargo-deny reported an unmatched advisory that the registry does not lease: {identifier}"
        for identifier in sorted(unknown)
    ]
    failures.extend(
        f"exception {identifier} suppresses an advisory in {workspace} without "
        "claiming it; add the workspace to the exception or scope the ignore list"
        for identifier in sorted(applied - claimed)
    )
    if reproducible:
        failures.extend(
            f"exception {identifier} claims {workspace} but matches nothing there; "
            "remove the claim or delete the exception"
            for identifier in sorted(claimed - applied)
        )

    if failures:
        print(
            f"{workspace}: advisory exception claims diverge from cargo-deny",
            file=sys.stderr,
        )
        print("\n".join(f"  {failure}" for failure in failures), file=sys.stderr)
        return 1
    print(
        f"{workspace}: {len(applied)} of {len(leased)} advisory exceptions apply here, "
        f"all claimed ({workspaces.describe(reproducible)}, "
        f"evaluated {today.isoformat()})."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
