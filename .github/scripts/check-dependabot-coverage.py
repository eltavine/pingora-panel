#!/usr/bin/env python3
"""Require a Dependabot update entry for every ecosystem present in the repo.

Adding a Cargo workspace without telling Dependabot about it silently removes
that workspace from automated dependency updates, which is invisible until an
advisory lands there. Ecosystems are discovered from the working tree, so a new
workspace fails this guard until it is declared.

Governing another ecosystem is one `ECOSYSTEMS` entry.

The configuration is read with a deliberately narrow scanner rather than a YAML
parser, because the runner has only the standard library. It therefore requires
the canonical layout Dependabot documents: each entry opens with a two-space
`- package-ecosystem:` and carries `directory:` at four spaces. Any other layout
fails closed instead of being silently misread.
"""

from __future__ import annotations

import argparse
import re
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from policy import PolicyError, ci_yaml

MAPPING_ENTRY = re.compile(
    r'(?:(?P<double>"[a-z][a-z-]*")|'
    r"(?P<single>'[a-z][a-z-]*')|(?P<plain>[a-z][a-z-]*)):\s*(?P<value>.*)"
)
IGNORED_DIRECTORIES = frozenset({"target", "node_modules", ".git"})


@dataclass(frozen=True)
class Ecosystem:
    """One Dependabot ecosystem and how to find the directories it must cover."""

    name: str
    discover: Callable[[Path], set[str]]
    rationale: str


def _as_dependabot_directory(root: Path, path: Path) -> str:
    relative = path.parent.relative_to(root).as_posix()
    return "/" if relative == "." else f"/{relative}"


def _cargo_workspaces(root: Path) -> set[str]:
    """Every directory holding a Cargo manifest that defines a workspace."""
    directories: set[str] = set()
    for manifest in sorted(root.rglob("Cargo.toml")):
        if any(part in IGNORED_DIRECTORIES for part in manifest.relative_to(root).parts):
            continue
        try:
            with manifest.open("rb") as source:
                document = tomllib.load(source)
        except (OSError, tomllib.TOMLDecodeError) as error:
            raise PolicyError(f"cannot read {manifest}: {error}") from error
        workspace = document.get("workspace")
        if isinstance(workspace, dict) and "members" in workspace:
            directories.add(_as_dependabot_directory(root, manifest))
    if not directories:
        raise PolicyError("no Cargo workspace roots were discovered")
    return directories


def _github_actions(root: Path) -> set[str]:
    """Workflows and composite Actions always live at the repository root."""
    if not (root / ".github/workflows").is_dir():
        raise PolicyError("no .github/workflows directory was discovered")
    return {"/"}


def mapping_entry(line: str, indent: int, *, sequence: bool = False) -> tuple[str, str] | None:
    """Read one canonical mapping entry at an exact indentation level."""
    prefix = " " * indent + ("- " if sequence else "")
    if not line.startswith(prefix):
        return None
    match = MAPPING_ENTRY.fullmatch(line[len(prefix) :])
    if match is None:
        return None
    raw_key = match.group("double") or match.group("single") or match.group("plain")
    key = raw_key[1:-1] if raw_key[:1] in {'"', "'"} else raw_key
    return key, match.group("value").strip()


def scalar(value: str, context: str) -> str:
    """Read one non-empty plain or simply quoted Dependabot scalar."""
    if not value:
        raise PolicyError(f"{context} must not be empty")
    if value[0] in {'"', "'"}:
        if len(value) < 2 or value[-1] != value[0]:
            raise PolicyError(f"{context} has mismatched quotes")
        return value[1:-1]
    if value[-1] in {'"', "'"}:
        raise PolicyError(f"{context} has mismatched quotes")
    return value


ECOSYSTEMS: tuple[Ecosystem, ...] = (
    Ecosystem("cargo", _cargo_workspaces, "every Cargo workspace root"),
    Ecosystem("github-actions", _github_actions, "the workflow directory"),
)


def declared_entries(configuration: Path) -> dict[str, set[str]]:
    """Read `updates[]` as a mapping from ecosystem to declared directories."""
    try:
        lines = [
            ci_yaml.strip_comment(line).rstrip()
            for line in configuration.read_text(encoding="utf-8").splitlines()
        ]
    except (OSError, UnicodeError) as error:
        raise PolicyError(f"cannot read {configuration}: {error}") from error

    root_declarations: dict[str, tuple[int, str]] = {}
    for index, line in enumerate(lines):
        if not line.strip() or line[0].isspace():
            continue
        parsed = mapping_entry(line, 0)
        if parsed is None:
            raise PolicyError(f"{configuration} has a malformed top-level entry")
        key, value = parsed
        if key in {"version", "updates"}:
            if key in root_declarations:
                raise PolicyError(f"{configuration} declares top-level {key} more than once")
            root_declarations[key] = (index, value)

    if "version" not in root_declarations or scalar(
        root_declarations["version"][1], "Dependabot version"
    ) != "2":
        raise PolicyError(f"{configuration} must declare exactly version: 2")
    if "updates" not in root_declarations or root_declarations["updates"][1]:
        raise PolicyError(f"{configuration} updates must be one block sequence")

    updates_start = root_declarations["updates"][0]
    updates_end = next(
        (
            index
            for index in range(updates_start + 1, len(lines))
            if lines[index].strip() and not lines[index][0].isspace()
        ),
        len(lines),
    )

    declared: dict[str, set[str]] = {}
    ecosystem: str | None = None
    directory: str | None = None

    def close() -> None:
        nonlocal ecosystem, directory
        if ecosystem is None:
            return
        if directory is None:
            raise PolicyError(f"{configuration} has a {ecosystem} entry without a directory")
        if directory in declared.setdefault(ecosystem, set()):
            raise PolicyError(
                f"{configuration} declares {ecosystem} for {directory} more than once"
            )
        declared[ecosystem].add(directory)
        ecosystem, directory = None, None

    for line in lines[updates_start + 1 : updates_end]:
        if not line.strip():
            continue
        indent = len(line) - len(line.lstrip())
        start = mapping_entry(line, 2, sequence=True) if indent == 2 else None
        if indent == 2:
            close()
            if start is None or start[0] != "package-ecosystem":
                raise PolicyError(
                    f"{configuration} update entries must begin with package-ecosystem"
                )
            ecosystem = scalar(start[1], "Dependabot package-ecosystem")
            continue
        field = mapping_entry(line, 4) if indent == 4 else None
        if indent == 4 and field is None:
            raise PolicyError(f"{configuration} has a malformed update field")
        if field is not None and field[0] == "directory":
            if ecosystem is None:
                raise PolicyError(f"{configuration} declares a directory outside an entry")
            if directory is not None:
                raise PolicyError(
                    f"{configuration} declares directory more than once in one entry"
                )
            directory = scalar(field[1], "Dependabot directory")
            continue
        if field is not None and field[0] == "package-ecosystem":
            raise PolicyError(
                f"{configuration} declares package-ecosystem outside an entry start"
            )
        if line.lstrip().startswith(("directory:", '"directory":', "'directory':")):
            raise PolicyError(f"{configuration} places directory at an unsupported indentation")
        if line.lstrip().startswith(
            ("package-ecosystem:", '"package-ecosystem":', "'package-ecosystem':")
        ):
            raise PolicyError(
                f"{configuration} places package-ecosystem at an unsupported indentation"
            )
    close()

    if not declared:
        raise PolicyError(f"{configuration} declares no update entries")
    return declared


def violations(root: Path, configuration: Path) -> list[str]:
    declared = declared_entries(configuration)
    failures: list[str] = []
    for ecosystem in ECOSYSTEMS:
        required = ecosystem.discover(root)
        covered = declared.get(ecosystem.name, set())
        failures.extend(
            f"{ecosystem.name} is not updated for {directory} "
            f"(policy requires {ecosystem.rationale})"
            for directory in sorted(required - covered)
        )
        failures.extend(
            f"{ecosystem.name} declares {directory}, which is not "
            f"{ecosystem.rationale}"
            for directory in sorted(covered - required)
        )
    unknown = sorted(set(declared) - {ecosystem.name for ecosystem in ECOSYSTEMS})
    failures.extend(
        f"{name} is declared but this guard does not govern it; "
        "add it to ECOSYSTEMS so its coverage is checked"
        for name in unknown
    )
    return failures


def main(argv: list[str] | None = None) -> int:
    repo_root = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("repository_root", nargs="?", type=Path, default=repo_root)
    parser.add_argument("--configuration", type=Path)
    arguments = parser.parse_args(argv)
    try:
        root = arguments.repository_root.resolve(strict=True)
        configuration = (
            arguments.configuration
            if arguments.configuration is not None
            else root / ".github/dependabot.yml"
        )
        failures = violations(root, configuration.resolve())
    except (OSError, PolicyError) as error:
        print(f"Dependabot coverage policy failed closed: {error}", file=sys.stderr)
        return 2
    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    governed = ", ".join(ecosystem.name for ecosystem in ECOSYSTEMS)
    print(f"Dependabot covers every discovered ecosystem ({governed}).")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
