#!/usr/bin/env python3
"""Make cargo-deny's advisory warnings blocking unless they are accounted for.

cargo-deny reports duplicate crates, unmaintained crates, and yanked crates as
warnings, so none of them fail a build on their own. This guard makes each one
blocking unless the workspace's declared enforcement accounts for it.

The finding kinds live in `LEASED_KINDS`, and how a workspace is governed lives
in `policy.finding_leases`; neither requires editing this script.
"""

from __future__ import annotations

import argparse
import sys
from dataclasses import dataclass
from datetime import date, datetime, timezone
from pathlib import Path

from policy import PolicyError, cargo_deny, fields, finding_leases, registry, workspaces
from policy.cargo_deny import FindingKindRegistry
from policy.finding_leases import Enforcement, LeasedKind

CHECKS = ("advisories", "bans")
DEFAULT_REGISTRY = ".github/policies/rust-dependency-leases.json"

LEASED_KINDS: tuple[LeasedKind, ...] = (
    LeasedKind(cargo_deny.DUPLICATE, identified_by_advisory=False, minimum_versions=2),
    LeasedKind(cargo_deny.UNMAINTAINED, identified_by_advisory=True),
    LeasedKind(cargo_deny.YANKED, identified_by_advisory=False),
)

KINDS = FindingKindRegistry(*(leased.kind for leased in LEASED_KINDS))


@dataclass(frozen=True)
class Evaluation:
    """The policy date and the repository the registry is evaluated against."""

    today: date
    repo_root: Path


DOCUMENTS: registry.DocumentRegistry[Evaluation, dict[str, Enforcement]] = (
    registry.DocumentRegistry("dependency lease registry")
)


@DOCUMENTS.reader(1)
def _read_v1(document: dict, evaluation: Evaluation) -> dict[str, Enforcement]:
    """Read the v1 contract: one declared enforcement per workspace manifest."""
    if set(document) != {"schema_version", "workspaces"}:
        raise PolicyError(
            "dependency lease registry v1 must contain exactly schema_version and workspaces"
        )
    declared = document["workspaces"]
    if not isinstance(declared, dict) or not declared:
        raise PolicyError("dependency lease registry must classify at least one workspace")
    governed: dict[str, Enforcement] = {}
    for manifest, entry in declared.items():
        key = fields.manifest_path(manifest)
        if not isinstance(entry, dict):
            raise PolicyError(f"dependency lease entry for {key} must be a mapping")
        governed[key] = finding_leases.read(
            key,
            entry,
            LEASED_KINDS,
            evaluation.today,
            workspaces.resolves_reproducibly(evaluation.repo_root, key),
        )
    return governed


def main(argv: list[str] | None = None) -> int:
    repo_root = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("workspace", help="repository-relative workspace manifest path")
    parser.add_argument("--registry", type=Path, default=repo_root / DEFAULT_REGISTRY)
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
        root = arguments.repo_root.resolve(strict=True)
        governed = DOCUMENTS.load(arguments.registry, Evaluation(today, root))
        enforcement = governed.get(workspace)
        if enforcement is None:
            raise PolicyError(
                f"dependency lease registry does not classify workspace {workspace}"
            )
        lines = (
            arguments.report.read_text(encoding="utf-8").splitlines()
            if arguments.report is not None
            else cargo_deny.run(root / workspace, root, CHECKS)
        )
        observed = cargo_deny.parse_report(lines, KINDS, CHECKS)
    except (OSError, UnicodeError, PolicyError) as error:
        print(f"Rust dependency lease policy failed closed: {error}", file=sys.stderr)
        return 2

    failures = enforcement.failures(observed)
    if failures:
        print(
            f"{workspace}: cargo-deny findings are not accounted for",
            file=sys.stderr,
        )
        print("\n".join(f"  {failure}" for failure in failures), file=sys.stderr)
        return 1
    print(
        f"{workspace}: {enforcement.summary(observed)} "
        f"(evaluated {today.isoformat()})."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
