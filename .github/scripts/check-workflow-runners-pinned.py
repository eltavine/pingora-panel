#!/usr/bin/env python3
"""Reject floating GitHub-hosted runner aliases in workflow files."""

from __future__ import annotations

import re
from pathlib import Path

from policy import PolicyError, ci_yaml, cli


FLOATING_RUNNER = re.compile(r"\b(?:ubuntu|macos|windows)-latest\b")


def workflow_files(root: Path) -> list[Path]:
    if not root.is_dir():
        raise PolicyError(f"workflow directory does not exist: {root}")
    workflows = sorted((*root.glob("*.yml"), *root.glob("*.yaml")))
    if not workflows:
        raise PolicyError(f"workflow directory contains no YAML files: {root}")
    return workflows


def violations(root: Path) -> list[str]:
    failures: list[str] = []
    for workflow in workflow_files(root):
        for line_number, line in enumerate(
            workflow.read_text(encoding="utf-8").splitlines(), start=1
        ):
            match = FLOATING_RUNNER.search(ci_yaml.strip_comment(line))
            if match is not None:
                failures.append(
                    f"{workflow}:{line_number}: floating runner label {match.group(0)!r}"
                )
    return failures


def main(argv: list[str] | None = None) -> int:
    entry = cli.Entrypoint("runner label guard", __doc__, dated=False)
    entry.parser.add_argument(
        "workflow_root",
        nargs="?",
        type=Path,
        default=Path(".github/workflows"),
    )
    arguments = entry.parse(argv)
    try:
        failures = violations(arguments.workflow_root)
    except cli.FAILING as error:
        return entry.failed_closed(error)
    return entry.report(
        failures, "GitHub-hosted runner labels avoid floating -latest aliases"
    )


if __name__ == "__main__":
    raise SystemExit(main())
