#!/usr/bin/env python3
"""Contract tests for proving the declared minimum Rust version is compiled.

Both directions of the drift are covered: a lane that moved past the declared
minimum, and a declared minimum that moved past its lane. Either one leaves the
promise in `rust-version` untested, which is the failure this guard exists for.
"""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path

CHECKER = Path(__file__).resolve().with_name("check-rust-msrv-tested.py")


def build_tree(
    root: Path,
    *,
    rust_version: str = "1.88",
    lane: str | None = "1.88.0 # rust-msrv-policy: panel-msrv",
    marker: str = "panel-msrv",
    manifest: str = "panel/Cargo.toml",
) -> tuple[Path, Path]:
    """Lay out a repository whose shape mirrors the real one."""
    (root / "panel").mkdir(parents=True, exist_ok=True)
    (root / "panel" / "Cargo.toml").write_text(
        "[workspace]\n"
        'members = []\n'
        "\n[workspace.package]\n"
        f'rust-version = "{rust_version}"\n',
        encoding="utf-8",
    )
    workflows = root / ".github" / "workflows"
    workflows.mkdir(parents=True, exist_ok=True)
    lane_line = f"            toolchain: {lane}\n" if lane is not None else ""
    (workflows / "panel.yml").write_text(
        "name: Panel\n"
        "jobs:\n"
        "  rust:\n"
        "    strategy:\n"
        "      matrix:\n"
        "        include:\n"
        "          - profile: msrv\n" + lane_line + "          - profile: stable\n"
        "            toolchain: 1.98.0\n",
        encoding="utf-8",
    )
    registry = root / "registry.json"
    registry.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "workspaces": [{"manifest": manifest, "marker": marker}],
            }
        ),
        encoding="utf-8",
    )
    return registry, workflows


def check(root: Path, registry: Path, workflows: Path) -> int:
    return subprocess.run(
        [
            sys.executable,
            str(CHECKER),
            "--repo-root",
            str(root),
            "--registry",
            str(registry),
            "--workflow-root",
            str(workflows),
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    ).returncode


def expect(actual: int, expected: int, scenario: str) -> None:
    if actual != expected:
        raise AssertionError(f"{scenario}: expected exit {expected}, got {actual}")


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="rust-msrv-policy.") as temporary:
        base = Path(temporary)

        def scenario(name: str, expected: int, **kwargs) -> None:
            root = base / name
            registry, workflows = build_tree(root, **kwargs)
            expect(check(root, registry, workflows), expected, name)

        scenario("the declared minimum is the compiled one", 0)
        # `rust-version = "1.88"` names the 1.88.0 release, so the comparison is
        # between releases rather than between strings.
        scenario("an equivalent shorter spelling", 0, rust_version="1.88.0")

        scenario(
            "a lane raised past the declared minimum",
            1,
            lane="1.90.0 # rust-msrv-policy: panel-msrv",
        )
        scenario(
            "a declared minimum raised past its lane",
            1,
            rust_version="1.92",
        )
        scenario("a workspace with no marked lane", 1, lane="1.88.0")
        scenario("a marked lane the registry does not govern", 1, marker="other-msrv")

        scenario("a manifest that does not exist", 2, manifest="absent/Cargo.toml")

        # A registry that cannot be read must never look like a pass.
        root = base / "malformed"
        registry, workflows = build_tree(root)
        registry.write_text("{not json", encoding="utf-8")
        expect(check(root, registry, workflows), 2, "a registry that is not JSON")

        root = base / "unsupported"
        registry, workflows = build_tree(root)
        registry.write_text(json.dumps({"schema_version": 99}), encoding="utf-8")
        expect(check(root, registry, workflows), 2, "an unsupported registry schema")

        root = base / "no-rust-version"
        registry, workflows = build_tree(root)
        (root / "panel" / "Cargo.toml").write_text(
            "[workspace]\nmembers = []\n", encoding="utf-8"
        )
        expect(check(root, registry, workflows), 2, "a manifest declaring no minimum")

    print("Rust MSRV policy self-test passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
