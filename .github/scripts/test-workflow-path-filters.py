#!/usr/bin/env python3
"""Contract tests for check-workflow-path-filters.py."""

from __future__ import annotations

import runpy
import subprocess
import tempfile
from pathlib import Path


CHECKER = Path(__file__).with_name("check-workflow-path-filters.py")
CHECKER_API = runpy.run_path(str(CHECKER), run_name="workflow_path_filter_checker")
path_is_included = CHECKER_API["path_is_included"]


def write_fixture(
    root: Path,
    patterns: list[str],
    *,
    include_run: bool = True,
    block_scalar: str = ">-",
    script_reference: str = ".github/scripts/check-example.sh",
    inline_paths: bool = False,
    filter_name: str = "paths",
    local_action: bool = False,
) -> None:
    workflow_root = root / ".github/workflows"
    script_root = root / ".github/scripts"
    workflow_root.mkdir(parents=True)
    script_root.mkdir(parents=True)
    (script_root / "check-example.sh").write_text("#!/usr/bin/env bash\n", encoding="utf-8")
    if local_action:
        action_root = root / ".github/actions/example"
        action_root.mkdir(parents=True)
        (action_root / "action.yml").write_text(
            "name: Example\nruns:\n  using: node24\n  main: index.js\n",
            encoding="utf-8",
        )
        (action_root / "index.js").write_text("// action fixture\n", encoding="utf-8")

    paths = "\n".join(f'      - "{pattern}"' for pattern in patterns)
    path_filter = (
        f"    {filter_name}: ["
        + ", ".join(f'"{pattern}"' for pattern in patterns)
        + "]\n"
        if inline_paths
        else f"    {filter_name}:\n{paths}\n"
    )
    run = (
        "      - name: Run local guard\n"
        f"        run: {block_scalar}\n"
        f"          bash {script_reference}\n"
        if include_run
        else ""
    )
    if local_action:
        run += "      - uses: ./.github/actions/example\n"
    (workflow_root / "check.yml").write_text(
        "name: Check\n\n"
        "on:\n"
        "  push:\n"
        f"{path_filter}\n"
        "jobs:\n"
        "  check:\n"
        "    runs-on: ubuntu-24.04\n"
        "    steps:\n"
        f"{run}",
        encoding="utf-8",
    )


def run_checker(root: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            "python3",
            str(CHECKER),
            "--repo-root",
            str(root),
            "--workflow-root",
            str(root / ".github/workflows"),
        ],
        check=False,
        capture_output=True,
        text=True,
    )


def assert_accepted(
    patterns: list[str], scenario: str, *, filter_name: str = "paths"
) -> None:
    with tempfile.TemporaryDirectory(prefix="workflow-paths-") as directory:
        root = Path(directory)
        write_fixture(root, patterns, filter_name=filter_name)
        result = run_checker(root)
        if result.returncode != 0:
            raise AssertionError(f"{scenario} was rejected: {result.stderr}")


def assert_rejected(
    patterns: list[str],
    expected: str,
    scenario: str,
    *,
    filter_name: str = "paths",
) -> None:
    with tempfile.TemporaryDirectory(prefix="workflow-paths-") as directory:
        root = Path(directory)
        write_fixture(root, patterns, filter_name=filter_name)
        result = run_checker(root)
        if result.returncode == 0:
            raise AssertionError(f"{scenario} was not rejected")
        if expected not in result.stderr:
            raise AssertionError(
                f"{scenario} did not report {expected!r}: {result.stderr}"
            )


def main() -> None:
    assert_accepted(
        [".github/scripts/check-example.sh", ".github/workflows/check.yml"],
        "exact path filters",
    )
    assert_accepted([".github/**"], "wildcard path filter")
    assert_accepted(["docs/**"], "unrelated ignored path", filter_name="paths-ignore")
    with tempfile.TemporaryDirectory(prefix="workflow-paths-") as directory:
        root = Path(directory)
        write_fixture(root, [".github/**"], block_scalar="|+")
        result = run_checker(root)
        if result.returncode != 0:
            raise AssertionError(f"block scalar modifiers were rejected: {result.stderr}")
    with tempfile.TemporaryDirectory(prefix="workflow-paths-") as directory:
        root = Path(directory)
        write_fixture(root, [".github/**"], inline_paths=True)
        result = run_checker(root)
        if result.returncode == 0 or "inline paths syntax" not in result.stderr:
            raise AssertionError(
                f"inline path syntax was not rejected safely: {result.stderr}"
            )
    assert_rejected(
        [".github/workflows/check.yml"],
        "does not cover .github/scripts/check-example.sh",
        "missing local dependency",
    )
    assert_rejected(
        [".github/scripts/check-example.sh"],
        "does not cover .github/workflows/check.yml",
        "missing workflow self-trigger",
    )
    assert_rejected(
        [".github/**", "!.github/scripts/check-example.sh"],
        "does not cover .github/scripts/check-example.sh",
        "negated local dependency",
    )
    assert_rejected(
        [".github/**"],
        "paths-ignore excludes .github/scripts/check-example.sh",
        "ignored local dependency",
        filter_name="paths-ignore",
    )
    with tempfile.TemporaryDirectory(prefix="workflow-paths-") as directory:
        root = Path(directory)
        write_fixture(
            root,
            [".github/scripts/check-example.sh", ".github/workflows/check.yml"],
            local_action=True,
        )
        result = run_checker(root)
        if result.returncode == 0 or ".github/actions/example/action.yml" not in result.stderr:
            raise AssertionError(
                f"local action contents were not enforced: {result.stderr}"
            )
    with tempfile.TemporaryDirectory(prefix="workflow-paths-") as directory:
        root = Path(directory)
        write_fixture(root, [".github/**"], local_action=True)
        result = run_checker(root)
        if result.returncode != 0:
            raise AssertionError(f"covered local action was rejected: {result.stderr}")

    glob_contracts = (
        ("README.md", ["**/README.md"], True, "zero-directory double star"),
        ("docs/README.md", ["docs/**/*.md"], True, "zero nested directories"),
        ("docs/a/README.md", ["docs/**/*.md"], True, "nested directories"),
        ("page.js", ["*.jsx?"], True, "optional preceding character"),
        ("page.jsx", ["*.jsx?"], True, "present optional character"),
        ("page.jsxx", ["*.jsx?"], False, "optional character upper bound"),
        ("release/v1.20", ["release/v[0-9].[0-9]+"], True, "class repetition"),
    )
    for path, patterns, expected, scenario in glob_contracts:
        actual = path_is_included(path, patterns)
        if actual is not expected:
            raise AssertionError(
                f"{scenario}: expected {expected} for {path!r} against {patterns!r}"
            )
    print("Workflow path-filter guard self-test passed.")


if __name__ == "__main__":
    main()
