#!/usr/bin/env python3
"""Enforce one exact workspace catalog for every reusable Panel crate."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tomllib
from collections.abc import Iterator, Mapping
from pathlib import Path
from typing import Any

from policy import PolicyError, registry


DEPENDENCY_TABLES = ("dependencies", "dev-dependencies", "build-dependencies")
FORBIDDEN_INHERITED_KEYS = frozenset(
    {"branch", "git", "path", "registry", "rev", "tag", "version"}
)


def read_toml(path: Path) -> dict[str, Any]:
    try:
        with path.open("rb") as source:
            document = tomllib.load(source)
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise PolicyError(f"cannot read TOML {path}: {error}") from error
    if not isinstance(document, dict):
        raise PolicyError(f"TOML root is not a table: {path}")
    return document


DOCUMENTS: registry.DocumentRegistry[None, dict[str, Any]] = registry.DocumentRegistry(
    "boundary policy"
)


@DOCUMENTS.reader(1)
def _read_v1(document: dict, _context: None) -> dict[str, Any]:
    if set(document) != {"schema_version", "members", "rules"}:
        raise PolicyError(
            "boundary policy v1 must contain exactly schema_version, members, and rules"
        )
    members = document.get("members")
    rules = document.get("rules")
    if not isinstance(members, dict) or not members or not isinstance(rules, dict):
        raise PolicyError("boundary policy must define members")
    for name, member in members.items():
        if (
            not isinstance(name, str)
            or not name
            or not isinstance(member, dict)
            or set(member) != {"catalog", "allowed_workspace_dependencies"}
            or not isinstance(member.get("catalog"), bool)
        ):
            raise PolicyError("boundary policy member catalog flags are malformed")
        allowed = member.get("allowed_workspace_dependencies")
        if (
            not isinstance(allowed, list)
            or not all(isinstance(value, str) and value for value in allowed)
            or len(allowed) != len(set(allowed))
        ):
            raise PolicyError("boundary policy member dependencies are malformed")
    return document


def cargo_metadata(manifest: Path) -> dict[str, Any]:
    if not manifest.is_file():
        raise PolicyError(f"Panel workspace manifest does not exist: {manifest}")
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
            cwd=manifest.parent,
            check=True,
            capture_output=True,
            text=True,
        )
        document = json.loads(completed.stdout)
    except (OSError, subprocess.CalledProcessError, json.JSONDecodeError) as error:
        detail = getattr(error, "stderr", "") or str(error)
        raise PolicyError(f"cannot read Cargo metadata: {detail.strip()}") from error
    if not isinstance(document, dict):
        raise PolicyError("Cargo metadata root is malformed")
    return document


def workspace_packages(metadata: Mapping[str, Any]) -> dict[str, dict[str, Any]]:
    member_ids = metadata.get("workspace_members")
    packages = metadata.get("packages")
    if not isinstance(member_ids, list) or not isinstance(packages, list):
        raise PolicyError("Cargo metadata has no workspace package list")
    selected: dict[str, dict[str, Any]] = {}
    for package in packages:
        if not isinstance(package, dict) or package.get("id") not in member_ids:
            continue
        name = package.get("name")
        manifest = package.get("manifest_path")
        version = package.get("version")
        if not all(isinstance(value, str) and value for value in (name, manifest, version)):
            raise PolicyError("Cargo metadata contains a malformed workspace package")
        if name in selected:
            raise PolicyError(f"workspace contains duplicate package name: {name}")
        selected[name] = {
            "manifest_path": Path(manifest).resolve(),
            "root": Path(manifest).resolve().parent,
            "version": version,
        }
    return selected


def dependency_tables(document: Mapping[str, Any]) -> Iterator[tuple[str, Mapping[str, Any]]]:
    for table_name in DEPENDENCY_TABLES:
        table = document.get(table_name, {})
        if not isinstance(table, dict):
            raise PolicyError(f"{table_name} must be a table")
        yield table_name, table

    targets = document.get("target", {})
    if not isinstance(targets, dict):
        raise PolicyError("target dependencies must be a table")
    for target_name, target in targets.items():
        if not isinstance(target, dict):
            raise PolicyError(f"target.{target_name} must be a table")
        for table_name in DEPENDENCY_TABLES:
            table = target.get(table_name, {})
            if not isinstance(table, dict):
                raise PolicyError(f"target.{target_name}.{table_name} must be a table")
            yield f"target.{target_name}.{table_name}", table


def dependency_actual_name(alias: str, declaration: object) -> str:
    if isinstance(declaration, dict):
        package = declaration.get("package", alias)
        if not isinstance(package, str) or not package:
            raise PolicyError(f"dependency {alias} has an invalid package name")
        return package
    if not isinstance(declaration, str):
        raise PolicyError(f"dependency {alias} must be a string or table")
    return alias


def canonical_dependency_path(workspace_root: Path, value: object) -> Path | None:
    if not isinstance(value, dict):
        return None
    raw_path = value.get("path")
    if not isinstance(raw_path, str) or not raw_path:
        return None
    return (workspace_root / raw_path).resolve()


def violations(
    manifest: Path, policy_path: Path, metadata: Mapping[str, Any]
) -> list[str]:
    policy = DOCUMENTS.load(policy_path, None)
    packages = workspace_packages(metadata)
    policy_members = set(policy["members"])
    workspace_members = set(packages)
    failures: list[str] = []
    if policy_members != workspace_members:
        missing = sorted(policy_members - workspace_members)
        extra = sorted(workspace_members - policy_members)
        failures.append(
            f"workspace/policy members differ (missing={missing}, unclassified={extra})"
        )

    root = read_toml(manifest)
    workspace = root.get("workspace")
    if not isinstance(workspace, dict):
        raise PolicyError("Panel root manifest has no [workspace] table")
    catalog = workspace.get("dependencies")
    if not isinstance(catalog, dict):
        raise PolicyError("Panel root manifest has no [workspace.dependencies] table")
    workspace_root = manifest.parent.resolve()

    catalog_actual_names: dict[str, str] = {}
    for alias, declaration in catalog.items():
        if not isinstance(alias, str):
            raise PolicyError("workspace dependency name is malformed")
        actual_name = dependency_actual_name(alias, declaration)
        catalog_actual_names[alias] = actual_name
        if actual_name in workspace_members and alias != actual_name:
            failures.append(
                f"internal catalog entry {alias} must use canonical member name {actual_name}"
            )

    for name, member_policy in policy["members"].items():
        if name not in packages:
            continue
        declaration = catalog.get(name)
        if not member_policy["catalog"]:
            if any(actual == name for actual in catalog_actual_names.values()):
                failures.append(f"non-reusable member {name} must not enter the catalog")
            continue
        if declaration is None:
            failures.append(f"reusable member {name} is missing from workspace.dependencies")
            continue
        if not isinstance(declaration, dict):
            failures.append(f"catalog entry {name} must be an inline dependency table")
            continue
        expected_version = f"={packages[name]['version']}"
        if declaration.get("version") != expected_version:
            failures.append(
                f"catalog entry {name} must use exact version {expected_version}"
            )
        resolved_path = canonical_dependency_path(workspace_root, declaration)
        if resolved_path != packages[name]["root"]:
            failures.append(
                f"catalog entry {name} must point to {packages[name]['root']}"
            )
        if declaration.get("package", name) != name:
            failures.append(f"catalog entry {name} must not redirect its package name")

    for package_name, package in packages.items():
        document = read_toml(package["manifest_path"])
        for table_name, table in dependency_tables(document):
            for alias, declaration in table.items():
                actual_name = dependency_actual_name(alias, declaration)
                if isinstance(declaration, dict) and declaration.get("workspace") is True:
                    catalog_name = catalog_actual_names.get(alias)
                    if catalog_name is None:
                        failures.append(
                            f"{package_name}:{table_name}.{alias} inherits a missing catalog entry"
                        )
                        continue
                    actual_name = catalog_name
                if actual_name not in workspace_members:
                    continue
                location = f"{package_name}:{table_name}.{alias}"
                if not isinstance(declaration, dict) or declaration.get("workspace") is not True:
                    failures.append(
                        f"{location} must inherit internal dependency {actual_name} with workspace = true"
                    )
                    continue
                forbidden = sorted(FORBIDDEN_INHERITED_KEYS.intersection(declaration))
                if forbidden:
                    failures.append(
                        f"{location} overrides centralized keys: {', '.join(forbidden)}"
                    )
                if alias not in catalog:
                    failures.append(f"{location} has no matching workspace catalog key")
                elif catalog_actual_names[alias] != actual_name:
                    failures.append(
                        f"{location} resolves to {actual_name}, but catalog resolves to "
                        f"{catalog_actual_names[alias]}"
                    )
    return failures


def main(argv: list[str] | None = None) -> int:
    repo_root = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "manifest", nargs="?", type=Path, default=repo_root / "panel/Cargo.toml"
    )
    parser.add_argument(
        "--policy",
        type=Path,
        default=repo_root / ".github/policies/panel-boundaries.json",
    )
    arguments = parser.parse_args(argv)
    manifest = arguments.manifest.resolve()
    try:
        failures = violations(
            manifest, arguments.policy.resolve(), cargo_metadata(manifest)
        )
    except PolicyError as error:
        print(f"Panel workspace dependency policy failed closed: {error}", file=sys.stderr)
        return 2
    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print("Panel workspace dependency catalog verified.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
