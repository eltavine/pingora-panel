#!/usr/bin/env python3
"""List canonical Cargo workspace package names from validated metadata."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path


class MetadataError(ValueError):
    """A malformed manifest or Cargo metadata response."""


def workspace_package_names(manifest: Path) -> tuple[str, ...]:
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
    if not names:
        raise MetadataError("Cargo workspace contains no packages")
    if discovered_member_ids != member_ids:
        raise MetadataError("Cargo metadata omits one or more workspace packages")
    if len(names) != len(set(names)):
        raise MetadataError("Cargo workspace contains duplicate package names")
    return tuple(sorted(names))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    arguments = parser.parse_args(argv)
    try:
        names = workspace_package_names(arguments.manifest.resolve())
    except MetadataError as error:
        print(f"workspace package discovery failed closed: {error}", file=sys.stderr)
        return 2
    print("\n".join(names))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
