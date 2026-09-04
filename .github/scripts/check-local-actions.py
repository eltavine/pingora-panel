#!/usr/bin/env python3
"""Validate repository-local GitHub Action references and manifests."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path, PurePosixPath

from policy import ci_yaml

from policy import PolicyError


USES = re.compile(
    r"(?:['\"]?uses['\"]?)\s*:\s*"
    r"(?P<value>\"[^\"]*\"|'[^']*'|[^\s,#}\]]+)"
)
BLOCK_USES = re.compile(r"^\s*(?:-\s*)?['\"]?uses['\"]?\s*:")
FLOW_PARENT = re.compile(
    r"^\s*(?:-\s*|['\"]?[A-Za-z0-9_-]+['\"]?\s*:\s*(?:\[\s*)?)\{"
)


def configuration_files(repo_root: Path) -> list[Path]:
    workflow_root = repo_root / ".github/workflows"
    if not workflow_root.is_dir():
        raise PolicyError(
            f"workflow directory does not exist: {workflow_root}"
        )
    files = set((*workflow_root.glob("*.yml"), *workflow_root.glob("*.yaml")))
    action_root = repo_root / ".github/actions"
    if action_root.is_dir():
        files.update(action_root.rglob("action.yml"))
        files.update(action_root.rglob("action.yaml"))
    if not files:
        raise PolicyError("no workflow or composite Action YAML files were found")
    return sorted(files)


def local_references(path: Path) -> list[tuple[int, str]]:
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeError) as error:
        raise PolicyError(f"cannot read {path}: {error}") from error
    references: list[tuple[int, str]] = []
    for line_number, raw_line in enumerate(lines, start=1):
        line = ci_yaml.strip_comment(raw_line)
        if not BLOCK_USES.match(line) and not FLOW_PARENT.match(line):
            continue
        for match in USES.finditer(line):
            value = match.group("value").strip()
            if len(value) >= 2 and value[0] == value[-1] and value[0] in {'"', "'"}:
                value = value[1:-1]
            if value.startswith("."):
                references.append((line_number, value))
    return references


def validate_reference(repo_root: Path, reference: str) -> str | None:
    if not reference.startswith("./"):
        return "local Action reference must start with './'"
    raw_path = reference[2:]
    relative = PurePosixPath(raw_path)
    if (
        not raw_path
        or "\\" in raw_path
        or relative.is_absolute()
        or relative.as_posix() != raw_path
        or any(part in {"", ".", ".."} for part in relative.parts)
    ):
        return "local Action path must be canonical and repository-relative"

    try:
        target = (repo_root / relative).resolve(strict=True)
        target.relative_to(repo_root)
    except (OSError, ValueError):
        return "local Action target is missing or escapes the repository"
    if not target.is_dir():
        return "local Action target must be a directory"

    manifests = [
        manifest
        for manifest in (target / "action.yml", target / "action.yaml")
        if manifest.is_file()
    ]
    if len(manifests) != 1:
        return "local Action target must contain exactly one action.yml or action.yaml"
    try:
        manifests[0].resolve(strict=True).relative_to(repo_root)
    except (OSError, ValueError):
        return "local Action manifest escapes the repository"
    return None


def violations(repo_root: Path) -> list[str]:
    failures: list[str] = []
    for configuration in configuration_files(repo_root):
        for line_number, reference in local_references(configuration):
            failure = validate_reference(repo_root, reference)
            if failure is not None:
                failures.append(
                    f"{configuration}:{line_number}: {failure}: {reference}"
                )
    return failures


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("repository_root", nargs="?", type=Path, default=Path.cwd())
    arguments = parser.parse_args(argv)
    try:
        repo_root = arguments.repository_root.resolve(strict=True)
        failures = violations(repo_root)
    except (OSError, PolicyError) as error:
        print(f"local Action policy failed closed: {error}", file=sys.stderr)
        return 2
    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print("Repository-local GitHub Action references are canonical and self-contained.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
