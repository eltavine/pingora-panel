#!/usr/bin/env python3
"""Enforce immutable, single-source package installations in CI YAML.

The installer rules live in `policy.installers` and the YAML scanning lives in
`policy.ci_yaml`, so governing a new installer never touches this script.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

from policy import PolicyError, ci_yaml, installers


def configuration_files(roots: list[Path]) -> list[Path]:
    files: set[Path] = set()
    for root in roots:
        if not root.is_dir():
            raise PolicyError(f"configuration directory does not exist: {root}")
        files.update(root.rglob("*.yml"))
        files.update(root.rglob("*.yaml"))
    if not files:
        raise PolicyError("no YAML configuration files were found")
    return sorted(files)


def violations(files: list[Path]) -> list[str]:
    reported: list[str] = []
    for path in files:
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as error:
            raise PolicyError(f"cannot read {path}: {error}") from error
        for block in ci_yaml.run_blocks(text):
            lines, split_failures = ci_yaml.logical_lines(block)
            reported.extend(f"{path}:{failure}" for failure in split_failures)
            for line in lines:
                reported.extend(
                    f"{path}:{line.number}: {failure}"
                    for failure in installers.failures(ci_yaml.shell_tokens(line.code))
                )
                for substitution in ci_yaml.shell_substitutions(line.code):
                    nested = installers.invocation_names(ci_yaml.shell_tokens(substitution))
                    reported.extend(
                        f"{path}:{line.number}: {name} must be a standalone command"
                        for name in nested
                    )
    return reported


def main(argv: list[str] | None = None) -> int:
    repo_root = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("configuration_root", nargs="?", type=Path)
    arguments = parser.parse_args(argv)
    roots = (
        [arguments.configuration_root.resolve()]
        if arguments.configuration_root is not None
        else [
            repo_root / ".github/workflows",
            *(
                [repo_root / ".github/actions"]
                if (repo_root / ".github/actions").is_dir()
                else []
            ),
        ]
    )
    try:
        failures = violations(configuration_files(roots))
    except PolicyError as error:
        print(f"CI tool pinning guard failed closed: {error}", file=sys.stderr)
        return 2
    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    governed = ", ".join(rule.name for rule in installers.RULES)
    print(f"CI installations are immutable and single-source ({governed}).")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
