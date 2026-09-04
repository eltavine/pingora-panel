#!/usr/bin/env python3
"""Contract tests for exact Rust toolchain and explicit canary enforcement."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path


CHECKER = Path(__file__).resolve().with_name("check-rust-toolchains-pinned.py")


def write_fixture(root: Path, workflow: str, allowance: bool = True) -> None:
    workflow_root = root / ".github/workflows"
    workflow_root.mkdir(parents=True)
    (workflow_root / "check.yml").write_text(workflow, encoding="utf-8")
    policy = {
        "schema_version": 1,
        "exact_versions": ["1.88.0", "1.98.0"],
        "floating_allowances": (
            [
                {
                    "workflow": ".github/workflows/check.yml",
                    "value": "nightly",
                    "marker": "test-canary",
                }
            ]
            if allowance
            else []
        ),
    }
    (root / "policy.json").write_text(json.dumps(policy), encoding="utf-8")


def check(root: Path) -> int:
    return subprocess.run(
        [
            sys.executable,
            str(CHECKER),
            "--repo-root",
            str(root),
            "--workflow-root",
            str(root / ".github/workflows"),
            "--action-root",
            str(root / ".github/actions"),
            "--policy",
            str(root / "policy.json"),
        ],
        check=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    ).returncode


def rewrite_policy(root: Path, update: dict[str, object]) -> None:
    path = root / "policy.json"
    policy = json.loads(path.read_text(encoding="utf-8"))
    policy.update(update)
    path.write_text(json.dumps(policy), encoding="utf-8")


def write_action(root: Path, contents: str, name: str = "toolchain") -> None:
    path = root / ".github/actions" / name / "action.yml"
    path.parent.mkdir(parents=True)
    path.write_text(contents, encoding="utf-8")


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="rust-toolchain-policy.") as temporary:
        root = Path(temporary)
        valid = root / "valid"
        write_fixture(
            valid,
            "jobs:\n  test:\n    strategy:\n      matrix:\n        include:\n"
            "          - toolchain: 1.88.0\n"
            "          - toolchain: nightly # rust-toolchain-policy: test-canary\n"
            "    steps:\n      - with:\n          toolchain: ${{ matrix.toolchain }}\n"
            "      - run: rustup toolchain install '1.88.0'\n"
            "      - run: rustup target add --toolchain=\"1.88.0\" wasm32-unknown-unknown\n",
        )
        write_action(
            valid,
            "name: exact toolchain\nruns:\n  using: composite\n  steps:\n"
            "    - shell: bash\n      run: cargo '+1.88.0' check\n",
        )
        if check(valid) != 0:
            raise AssertionError("exact and explicitly marked canary toolchains were rejected")
        (valid / ".github/workflows/check.yml").write_text(
            "jobs:\n  test:\n    toolchain: 1.88.0\n"
            "    # {toolchain: stable}\n"
            "    run: echo '# rust-toolchain-policy: not-a-marker'\n",
            encoding="utf-8",
        )
        rewrite_policy(valid, {"floating_allowances": []})
        if check(valid) != 0:
            raise AssertionError("commented and quoted toolchain text was rejected")

        composite_canary = root / "composite-canary"
        write_fixture(
            composite_canary,
            "jobs:\n  test:\n    toolchain: 1.88.0\n",
            allowance=False,
        )
        write_action(
            composite_canary,
            "name: canary toolchain\nruns:\n  using: composite\n  steps:\n"
            "    - shell: bash\n"
            "      run: cargo +nightly check # rust-toolchain-policy: action-canary\n",
        )
        rewrite_policy(
            composite_canary,
            {
                "floating_allowances": [
                    {
                        "workflow": ".github/actions/toolchain/action.yml",
                        "value": "nightly",
                        "marker": "action-canary",
                    }
                ]
            },
        )
        if check(composite_canary) != 0:
            raise AssertionError("declared composite-action canary was rejected")

        continued_canary = root / "continued-canary"
        write_fixture(
            continued_canary,
            "jobs:\n  test:\n    steps:\n      - run: |\n"
            "          cargo \\\n"
            "            +nightly test # rust-toolchain-policy: test-canary\n",
        )
        if check(continued_canary) != 0:
            raise AssertionError("declared continued canary command was rejected")

        folded_canary = root / "folded-canary"
        write_fixture(
            folded_canary,
            "jobs:\n  test:\n    steps:\n      - run: >-\n"
            "          cargo\n"
            "          +nightly test # rust-toolchain-policy: test-canary\n",
        )
        if check(folded_canary) != 0:
            raise AssertionError("declared folded canary command was rejected")

        scenarios = {
            "stable": "jobs:\n  test:\n    toolchain: stable\n",
            "spaced-key": "jobs:\n  test:\n    toolchain : stable\n",
            "quoted-key": 'jobs:\n  test:\n    "toolchain": stable\n',
            "flow-key": "jobs:\n  test:\n    strategy: {toolchain: stable}\n",
            "unmarked": "jobs:\n  test:\n    toolchain: nightly\n",
            "unknown-exact": "jobs:\n  test:\n    toolchain: 1.99.0\n",
            "inline": 'jobs:\n  test:\n    toolchain: ["1.88.0", nightly]\n',
            "cargo-override": "jobs:\n  test:\n    run: cargo +stable test\n",
            "quoted-cargo-override": (
                "jobs:\n  test:\n    run: cargo '+nightly' test\n"
            ),
            "continued-cargo-override": (
                "jobs:\n  test:\n    steps:\n      - run: |\n"
                "          cargo \\\n"
                "            +stable test\n"
            ),
            "folded-cargo-override": (
                "jobs:\n  test:\n    steps:\n      - run: >-\n"
                "          cargo\n"
                "          +stable test\n"
            ),
            "folded-rustup-env": (
                "jobs:\n  test:\n    steps:\n      - run: >-\n"
                "          RUSTUP_TOOLCHAIN=stable\n"
                "          cargo test\n"
            ),
            "rustup-env": "jobs:\n  test:\n    env:\n      RUSTUP_TOOLCHAIN: stable\n",
            "rustup-shell-env": (
                "jobs:\n  test:\n    run: RUSTUP_TOOLCHAIN=stable cargo test\n"
            ),
            "rustup-export-env": (
                "jobs:\n  test:\n    run: export RUSTUP_TOOLCHAIN=nightly\n"
            ),
            "rustup-install": (
                "jobs:\n  test:\n    steps:\n"
                "      - run: rustup toolchain install stable\n"
            ),
            "rustup-install-alias": "jobs:\n  test:\n    run: rustup install stable\n",
            "rustup-update": "jobs:\n  test:\n    run: rustup update nightly\n",
            "rustup-default": "jobs:\n  test:\n    run: rustup default nightly\n",
            "quoted-rustup-default": (
                "jobs:\n  test:\n    run: rustup default 'nightly'\n"
            ),
            "rustup-override": (
                "jobs:\n  test:\n    run: rustup override set stable\n"
            ),
            "rustup-run": "jobs:\n  test:\n    run: rustup run beta cargo test\n",
            "rustup-flag": (
                "jobs:\n  test:\n"
                "    run: rustup target add --toolchain stable wasm32-unknown-unknown\n"
            ),
        }
        for name, workflow in scenarios.items():
            fixture = root / name
            write_fixture(fixture, workflow, allowance=False)
            if check(fixture) == 0:
                raise AssertionError(f"{name} floating toolchain was not rejected")

        composite_bypass = root / "composite-bypass"
        write_fixture(
            composite_bypass,
            "jobs:\n  test:\n    toolchain: 1.88.0\n",
            allowance=False,
        )
        write_action(
            composite_bypass,
            "name: floating toolchain\nruns:\n  using: composite\n  steps:\n"
            "    - shell: bash\n      run: rustup update stable\n",
        )
        if check(composite_bypass) == 0:
            raise AssertionError("composite-action floating toolchain was not rejected")

        wrong_marker = root / "wrong-marker"
        write_fixture(
            wrong_marker,
            "jobs:\n  test:\n    toolchain: nightly # rust-toolchain-policy: other\n",
        )
        if check(wrong_marker) == 0:
            raise AssertionError("undeclared canary marker was not rejected")

        empty = root / "empty"
        (empty / ".github/workflows").mkdir(parents=True)
        (empty / "policy.json").write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "exact_versions": ["1.88.0"],
                    "floating_allowances": [],
                }
            ),
            encoding="utf-8",
        )
        if check(empty) == 0:
            raise AssertionError("empty workflow directory was not rejected")

        malformed_policies = {
            "unknown-policy-field": {"owner": "nobody"},
            "escaping-workflow": {
                "floating_allowances": [
                    {
                        "workflow": ".github/workflows/../check.yml",
                        "value": "nightly",
                        "marker": "test-canary",
                    }
                ]
            },
            "malformed-toolchain": {
                "floating_allowances": [
                    {
                        "workflow": ".github/workflows/check.yml",
                        "value": "nightly; true",
                        "marker": "test-canary",
                    }
                ]
            },
            "malformed-marker": {
                "floating_allowances": [
                    {
                        "workflow": ".github/workflows/check.yml",
                        "value": "nightly",
                        "marker": "Test_Canary",
                    }
                ]
            },
        }
        for name, update in malformed_policies.items():
            fixture = root / name
            write_fixture(fixture, "jobs:\n  test:\n    toolchain: 1.88.0\n")
            rewrite_policy(fixture, update)
            if check(fixture) == 0:
                raise AssertionError(f"{name} policy was not rejected")

    print("Rust toolchain policy self-test passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
