#!/usr/bin/env python3
"""Produce the one cargo-deny report every dependency guard reads.

Each guard used to invoke cargo-deny itself. For a workspace whose lockfile is
not committed that is not merely wasteful: the two runs resolve the graph
independently, so two guards examining the same commit could disagree about
which crates are even in it, and the disagreement would surface as one guard
passing and the other reporting a finding nobody could reproduce.

Running once and handing both guards the same bytes removes the possibility.
The report is validated here, against the union of every finding kind the
repository classifies, so an unusable report fails at the step that produced it
rather than diffusely in whichever guard read it first.
"""

from __future__ import annotations

from pathlib import Path

from policy import cargo_deny, cli
from policy.cargo_deny import FindingKindRegistry

KINDS = FindingKindRegistry(*cargo_deny.KNOWN)


def emit(manifest: Path, repo_root: Path, destination: Path) -> tuple[int, int]:
    """Run cargo-deny once and write a report both guards can read.

    Returns the line and finding counts, so the producing step reports what it
    handed on rather than leaving the next step to discover an empty file.
    """
    lines = cargo_deny.run(manifest, repo_root, cargo_deny.ALL_CHECKS)
    findings = cargo_deny.parse_report(lines, KINDS, cargo_deny.ALL_CHECKS)
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return len(lines), len(findings)


def main(argv: list[str] | None = None) -> int:
    entry = cli.Entrypoint("cargo-deny report", __doc__, dated=False)
    entry.parser.add_argument("manifest", type=Path)
    entry.parser.add_argument("destination", type=Path)
    entry.add_repo_root()
    arguments = entry.parse(argv)

    try:
        lines, findings = emit(
            arguments.manifest, arguments.repo_root, arguments.destination
        )
    except cli.FAILING as error:
        return entry.failed_closed(error)

    return entry.report(
        [],
        f"{arguments.manifest}: wrote {lines} report lines carrying {findings} "
        f"classified findings to {arguments.destination}",
    )


if __name__ == "__main__":
    raise SystemExit(main())
