#!/usr/bin/env python3
"""Require the declared minimum Rust version to be the one CI compiles with.

A workspace's `rust-version` is a promise to anyone building it. Nothing makes
that promise true: the lane that would test it is an ordinary matrix entry, so
raising the toolchain there while leaving `rust-version` behind leaves the
claimed minimum untested, and raising `rust-version` while leaving the lane
behind claims a minimum the tree no longer needs.

The lane is identified by a marker comment rather than by its position or its
name, matching how canary lanes are already marked, so reordering a matrix or
renaming a job cannot silently detach the promise from its proof.

Governing another workspace is one entry in the registry.
"""

from __future__ import annotations

import re
import tomllib
from dataclasses import dataclass
from pathlib import Path

from policy import PolicyError, cli, fields, registry

DEFAULT_REGISTRY = ".github/policies/rust-msrv.json"
WORKFLOW_ROOT = ".github/workflows"

MARKED_TOOLCHAIN = re.compile(
    r"^\s*(?:-\s*)?toolchain\s*:\s*(?P<toolchain>\S+)"
    r"\s*#\s*rust-msrv-policy:\s*(?P<marker>[a-z0-9]+(?:-[a-z0-9]+)*)\s*$"
)
MARKER = re.compile(r"[a-z0-9]+(?:-[a-z0-9]+)*")
VERSION = re.compile(r"\d+(?:\.\d+){1,2}")


@dataclass(frozen=True)
class GovernedWorkspace:
    """One workspace whose declared minimum must be proved by a marked lane."""

    manifest: str
    marker: str


DOCUMENTS: registry.DocumentRegistry[None, tuple[GovernedWorkspace, ...]] = (
    registry.DocumentRegistry("MSRV registry")
)


@DOCUMENTS.reader(1)
def _read_v1(document: dict, _context: None) -> tuple[GovernedWorkspace, ...]:
    if set(document) != {"schema_version", "workspaces"}:
        raise PolicyError(
            "MSRV registry v1 must contain exactly schema_version and workspaces"
        )
    declared = document["workspaces"]
    if not isinstance(declared, list) or not declared:
        raise PolicyError("MSRV registry must govern at least one workspace")
    governed: list[GovernedWorkspace] = []
    seen: set[str] = set()
    for index, entry in enumerate(declared):
        context = f"MSRV registry workspace[{index}]"
        if not isinstance(entry, dict) or set(entry) != {"manifest", "marker"}:
            raise PolicyError(f"{context} must contain exactly manifest and marker")
        manifest = fields.manifest_path(entry["manifest"], f"{context} manifest")
        marker = fields.matching(entry["marker"], f"{context} marker", MARKER)
        if manifest in seen:
            raise PolicyError(f"MSRV registry governs {manifest} twice")
        seen.add(manifest)
        governed.append(GovernedWorkspace(manifest=manifest, marker=marker))
    return tuple(governed)


def normalized(version: str, context: str) -> str:
    """Expand a Cargo version requirement to the exact release it names.

    `rust-version = "1.88"` names the 1.88.0 release, so comparing it to a
    toolchain spelled `1.88.0` has to compare releases rather than strings.
    """
    candidate = fields.matching(version, context, VERSION)
    parts = candidate.split(".")
    return ".".join(parts + ["0"] * (3 - len(parts)))


def declared_minimum(repo_root: Path, manifest: str) -> str:
    """Read `rust-version` from a workspace manifest."""
    path = repo_root / manifest
    try:
        document = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as error:
        raise PolicyError(f"cannot read {manifest}: {error}") from error
    for table in ("workspace", "package"):
        section = document.get(table)
        if isinstance(section, dict):
            package = section.get("package") if table == "workspace" else section
            if isinstance(package, dict) and "rust-version" in package:
                return normalized(
                    package["rust-version"], f"{manifest} rust-version"
                )
    raise PolicyError(f"{manifest} declares no rust-version to prove")


def marked_lanes(workflow_root: Path) -> dict[str, list[tuple[str, str]]]:
    """Collect every marked toolchain lane, keyed by its marker."""
    lanes: dict[str, list[tuple[str, str]]] = {}
    for path in sorted(workflow_root.rglob("*.yml")) + sorted(
        workflow_root.rglob("*.yaml")
    ):
        try:
            lines = path.read_text(encoding="utf-8").splitlines()
        except (OSError, UnicodeError) as error:
            raise PolicyError(f"cannot read {path}: {error}") from error
        for number, line in enumerate(lines, start=1):
            match = MARKED_TOOLCHAIN.match(line)
            if match is None:
                continue
            location = f"{path.name}:{number}"
            lanes.setdefault(match.group("marker"), []).append(
                (location, match.group("toolchain"))
            )
    return lanes


def violations(
    repo_root: Path, workflow_root: Path, governed: tuple[GovernedWorkspace, ...]
) -> tuple[list[str], list[str]]:
    lanes = marked_lanes(workflow_root)
    failures: list[str] = []
    proved: list[str] = []
    for workspace in governed:
        minimum = declared_minimum(repo_root, workspace.manifest)
        marked = lanes.get(workspace.marker, [])
        if not marked:
            failures.append(
                f"{workspace.manifest} declares rust-version {minimum} but no lane "
                f"is marked `rust-msrv-policy: {workspace.marker}`, so the minimum "
                "is never compiled"
            )
            continue
        for location, toolchain in marked:
            actual = normalized(toolchain, f"{location} toolchain")
            if actual != minimum:
                failures.append(
                    f"{location} compiles the {workspace.marker} lane with "
                    f"{actual}, but {workspace.manifest} declares rust-version "
                    f"{minimum}; the declared minimum is never compiled"
                )
        proved.append(f"{workspace.manifest} at {minimum}")

    unclaimed = sorted(set(lanes) - {workspace.marker for workspace in governed})
    failures.extend(
        f"{lanes[marker][0][0]} is marked `rust-msrv-policy: {marker}`, which the "
        "registry does not govern"
        for marker in unclaimed
    )
    return failures, proved


def main(argv: list[str] | None = None) -> int:
    entry = cli.Entrypoint("Rust MSRV policy", __doc__, dated=False)
    entry.add_registry(DEFAULT_REGISTRY)
    entry.add_repo_root()
    entry.parser.add_argument("--workflow-root", type=Path)
    arguments = entry.parse(argv)

    try:
        repo_root = arguments.repo_root.resolve(strict=True)
        workflow_root = (
            arguments.workflow_root
            if arguments.workflow_root is not None
            else repo_root / WORKFLOW_ROOT
        ).resolve(strict=True)
        governed = DOCUMENTS.load(arguments.registry, None)
        failures, proved = violations(repo_root, workflow_root, governed)
    except cli.FAILING as error:
        return entry.failed_closed(error)

    return entry.report(
        failures,
        f"Every declared minimum Rust version is compiled in CI ({', '.join(proved)})",
        header="declared minimum Rust versions are not the ones CI compiles",
    )


if __name__ == "__main__":
    raise SystemExit(main())
