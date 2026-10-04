#!/usr/bin/env python3
"""List canonical Cargo workspace package names from validated metadata.

With --generated, list only the packages generated from another contract,
which declare it as `[package.metadata.pingora-panel] generated-from`; that
contract's own tooling owns their compatibility.

With --libraries, list only the packages with a library target, the only
kind of target with a Rust API.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path


LIBRARY_KINDS = frozenset({"lib", "rlib", "dylib"})


class MetadataError(ValueError):
    """A malformed manifest or Cargo metadata response."""


def workspace_package_names(
    manifest: Path, generated: bool = False, libraries: bool = False
) -> tuple[str, ...]:
    if not manifest.is_file():
        raise MetadataError(f"workspace manifest does not exist: {manifest}")
    try:
        completed = subprocess.run(
            [
                "cargo",
                "metadata",
                "--manifest-path",
                str(manifest),
                "--format-version",
                "1",
                "--no-deps",
                "--locked",
            ],
            check=True,
            capture_output=True,
            text=True,
        )
        metadata = json.loads(completed.stdout)
    except (OSError, subprocess.CalledProcessError, json.JSONDecodeError) as error:
        detail = getattr(error, "stderr", "") or str(error)
        raise MetadataError(f"cannot read Cargo metadata: {detail.strip()}") from error

    if not isinstance(metadata, dict):
        raise MetadataError("Cargo metadata root is not an object")
    members = metadata.get("workspace_members")
    packages = metadata.get("packages")
    if not isinstance(members, list) or not isinstance(packages, list):
        raise MetadataError("Cargo metadata has no workspace package collection")
    if (
        not members
        or not all(isinstance(member, str) and member for member in members)
        or len(members) != len(set(members))
    ):
        raise MetadataError("Cargo metadata contains malformed workspace member IDs")
    member_ids = set(members)
    discovered_member_ids: set[str] = set()
    names: list[str] = []
    selected: list[str] = []
    for package in packages:
        if not isinstance(package, dict):
            raise MetadataError("Cargo metadata contains a malformed package entry")
        package_id = package.get("id")
        if not isinstance(package_id, str) or not package_id:
            raise MetadataError("Cargo metadata contains a malformed package ID")
        if package_id not in member_ids:
            continue
        if package_id in discovered_member_ids:
            raise MetadataError("Cargo metadata repeats a workspace package ID")
        discovered_member_ids.add(package_id)
        name = package.get("name")
        if not isinstance(name, str) or not name or "\n" in name or "\r" in name:
            raise MetadataError("Cargo metadata contains a malformed package name")
        names.append(name)
        if generated and generated_from(package, name) is None:
            continue
        if libraries and not has_library(package, name):
            continue
        selected.append(name)
    if not names:
        raise MetadataError("Cargo workspace contains no packages")
    if discovered_member_ids != member_ids:
        raise MetadataError("Cargo metadata omits one or more workspace packages")
    if len(names) != len(set(names)):
        raise MetadataError("Cargo workspace contains duplicate package names")
    return tuple(sorted(selected))


def has_library(package: dict[str, object], name: str) -> bool:
    targets = package.get("targets")
    if not isinstance(targets, list) or not all(
        isinstance(target, dict) and isinstance(target.get("kind"), list) for target in targets
    ):
        raise MetadataError(f"package {name} has malformed targets")
    return any(LIBRARY_KINDS.intersection(target["kind"]) for target in targets)


def generated_from(package: dict[str, object], name: str) -> str | None:
    metadata = package.get("metadata")
    if metadata is None:
        return None
    if not isinstance(metadata, dict):
        raise MetadataError(f"package {name} has malformed metadata")
    panel = metadata.get("pingora-panel")
    if panel is None:
        return None
    if not isinstance(panel, dict):
        raise MetadataError(f"package {name} has malformed pingora-panel metadata")
    source = panel.get("generated-from")
    if source is not None and (not isinstance(source, str) or not source):
        raise MetadataError(f"package {name} has a malformed generated-from source")
    return source


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument(
        "--generated",
        action="store_true",
        help="list only packages generated from another contract",
    )
    parser.add_argument(
        "--libraries",
        action="store_true",
        help="list only packages with a library target",
    )
    arguments = parser.parse_args(argv)
    try:
        names = workspace_package_names(
            arguments.manifest.resolve(), arguments.generated, arguments.libraries
        )
    except MetadataError as error:
        print(f"workspace package discovery failed closed: {error}", file=sys.stderr)
        return 2
    if names:
        print("\n".join(names))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
