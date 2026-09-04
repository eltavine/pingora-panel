#!/usr/bin/env python3
"""Contract tests for repository-local GitHub Action reference validation."""

from __future__ import annotations

import subprocess
import sys
import tempfile
from pathlib import Path


CHECKER = Path(__file__).resolve().with_name("check-local-actions.py")

WORKFLOW_TEMPLATE = """\
name: local
on:
  push:
jobs:
  build:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4
      - uses: {reference}
"""

MANIFEST = """\
name: local action
runs:
  using: composite
  steps:
    - run: 'true'
      shell: bash
"""


def repository(root: Path) -> Path:
    (root / ".github/workflows").mkdir(parents=True)
    (root / ".github/actions").mkdir(parents=True)
    return root


def write_workflow(root: Path, reference: str, name: str = "local.yml") -> None:
    (root / ".github/workflows" / name).write_text(
        WORKFLOW_TEMPLATE.format(reference=reference), encoding="utf-8"
    )


def write_action(root: Path, relative: str, manifest: str = "action.yml") -> Path:
    directory = root / relative
    directory.mkdir(parents=True, exist_ok=True)
    (directory / manifest).write_text(MANIFEST, encoding="utf-8")
    return directory


def check(root: Path) -> int:
    return subprocess.run(
        [sys.executable, str(CHECKER), str(root)],
        check=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    ).returncode


def case(name: str, expected: int, build: object) -> None:
    with tempfile.TemporaryDirectory(prefix="local-actions.") as temporary:
        root = Path(temporary).resolve()
        build(root)  # type: ignore[operator]
        actual = check(root)
        if actual != expected:
            raise AssertionError(f"{name}: expected exit {expected}, got {actual}")


def main() -> int:
    def canonical(root: Path) -> None:
        repository(root)
        write_action(root, ".github/actions/valid")
        write_workflow(root, "./.github/actions/valid")

    def yaml_manifest(root: Path) -> None:
        repository(root)
        write_action(root, ".github/actions/valid", manifest="action.yaml")
        write_workflow(root, "./.github/actions/valid")

    def external_only(root: Path) -> None:
        repository(root)
        write_workflow(
            root,
            "actions/setup-go@b7ad1dad31e06c5925ef5d2fc7ad053ef454303e # v7",
        )

    def missing_target(root: Path) -> None:
        repository(root)
        write_workflow(root, "./.github/actions/absent")

    def parent_traversal(root: Path) -> None:
        repository(root)
        write_action(root, ".github/actions/valid")
        write_workflow(root, "./.github/actions/../actions/valid")

    def absolute_path(root: Path) -> None:
        repository(root)
        write_workflow(root, ".//etc")

    def windows_separator(root: Path) -> None:
        repository(root)
        write_action(root, ".github/actions/valid")
        write_workflow(root, "./.github\\actions\\valid")

    def dot_prefixed_non_local(root: Path) -> None:
        repository(root)
        write_action(root, ".github/actions/valid")
        write_workflow(root, ".\\.github\\actions\\valid")

    def sibling_traversal(root: Path) -> None:
        repository(root)
        write_workflow(root, "../actions/valid")

    def trailing_slash(root: Path) -> None:
        repository(root)
        write_action(root, ".github/actions/valid")
        write_workflow(root, "./.github/actions/valid/")

    def escaping_symlink(root: Path) -> None:
        repository(root)
        outside = root.parent / f"{root.name}-outside"
        (outside / "action").mkdir(parents=True)
        (outside / "action/action.yml").write_text(MANIFEST, encoding="utf-8")
        (root / ".github/actions/escape").symlink_to(
            outside / "action", target_is_directory=True
        )
        write_workflow(root, "./.github/actions/escape")

    def internal_symlink(root: Path) -> None:
        repository(root)
        write_action(root, ".github/actions/valid")
        (root / ".github/actions/alias").symlink_to(
            root / ".github/actions/valid", target_is_directory=True
        )
        write_workflow(root, "./.github/actions/alias")

    def escaping_manifest_symlink(root: Path) -> None:
        repository(root)
        outside = root.parent / f"{root.name}-manifest"
        outside.mkdir(parents=True)
        (outside / "action.yml").write_text(MANIFEST, encoding="utf-8")
        directory = root / ".github/actions/leaky"
        directory.mkdir(parents=True)
        (directory / "action.yml").symlink_to(outside / "action.yml")
        write_workflow(root, "./.github/actions/leaky")

    def file_target(root: Path) -> None:
        repository(root)
        (root / ".github/actions/file.yml").write_text(MANIFEST, encoding="utf-8")
        write_workflow(root, "./.github/actions/file.yml")

    def missing_manifest(root: Path) -> None:
        repository(root)
        (root / ".github/actions/empty").mkdir(parents=True)
        write_workflow(root, "./.github/actions/empty")

    def ambiguous_manifest(root: Path) -> None:
        repository(root)
        directory = write_action(root, ".github/actions/ambiguous")
        (directory / "action.yaml").write_text(MANIFEST, encoding="utf-8")
        write_workflow(root, "./.github/actions/ambiguous")

    def nested_composite_reference(root: Path) -> None:
        repository(root)
        write_action(root, ".github/actions/valid")
        composite = root / ".github/actions/outer"
        composite.mkdir(parents=True)
        (composite / "action.yml").write_text(
            "name: outer\nruns:\n  using: composite\n  steps:\n"
            "    - uses: ./.github/actions/absent\n",
            encoding="utf-8",
        )
        write_workflow(root, "./.github/actions/valid")

    def commented_reference(root: Path) -> None:
        repository(root)
        write_action(root, ".github/actions/valid")
        (root / ".github/workflows/local.yml").write_text(
            WORKFLOW_TEMPLATE.format(reference="./.github/actions/valid")
            + "      # - uses: ./.github/actions/absent\n",
            encoding="utf-8",
        )

    def no_workflow_directory(root: Path) -> None:
        (root / ".github").mkdir(parents=True)

    def empty_workflow_directory(root: Path) -> None:
        repository(root)

    accepted = {
        "canonical local Action": canonical,
        "action.yaml manifest": yaml_manifest,
        "external pinned Action only": external_only,
        "repository-internal symlink": internal_symlink,
        "commented-out reference": commented_reference,
    }
    rejected = {
        "missing target": missing_target,
        "parent traversal": parent_traversal,
        "absolute path": absolute_path,
        "Windows separators": windows_separator,
        "dot-prefixed non-local reference": dot_prefixed_non_local,
        "sibling traversal outside the repository": sibling_traversal,
        "trailing slash": trailing_slash,
        "symlink escaping the repository": escaping_symlink,
        "manifest symlink escaping the repository": escaping_manifest_symlink,
        "file target": file_target,
        "missing manifest": missing_manifest,
        "ambiguous manifest": ambiguous_manifest,
        "broken reference inside a composite Action": nested_composite_reference,
    }
    failed_closed = {
        "missing workflow directory": no_workflow_directory,
        "empty workflow directory": empty_workflow_directory,
    }

    for name, build in accepted.items():
        case(name, 0, build)
    for name, build in rejected.items():
        case(name, 1, build)
    for name, build in failed_closed.items():
        case(name, 2, build)

    print("Local GitHub Action policy self-test passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
