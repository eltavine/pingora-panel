#!/usr/bin/env python3
"""Contract tests for the Panel workspace dependency catalog guard."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path


CHECKER = Path(__file__).resolve().with_name("check-panel-workspace-dependencies.py")


def create_fixture(root: Path, mode: str) -> tuple[Path, Path]:
    (root / "core/src").mkdir(parents=True)
    (root / "app/src").mkdir(parents=True)
    (root / "Cargo.toml").write_text(
        '[workspace]\nresolver = "2"\nmembers = ["core", "app"]\n\n'
        '[workspace.dependencies]\ncore = { version = "=0.1.0", path = "core" }\n',
        encoding="utf-8",
    )
    (root / "core/Cargo.toml").write_text(
        '[package]\nname = "core"\nversion = "0.1.0"\nedition = "2021"\n',
        encoding="utf-8",
    )
    valid_app_manifest = (
        '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n'
        "\n[dependencies]\ncore.workspace = true\n"
    )
    (root / "app/Cargo.toml").write_text(valid_app_manifest, encoding="utf-8")
    (root / "core/src/lib.rs").write_text("// core fixture\n", encoding="utf-8")
    (root / "app/src/lib.rs").write_text("// app fixture\n", encoding="utf-8")
    policy = root / "policy.json"
    policy.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "members": {
                    "app": {
                        "catalog": False,
                        "allowed_workspace_dependencies": ["core"],
                    },
                    "core": {
                        "catalog": True,
                        "allowed_workspace_dependencies": [],
                    },
                },
                "rules": {},
            }
        ),
        encoding="utf-8",
    )
    subprocess.run(
        ["cargo", "generate-lockfile", "--manifest-path", str(root / "Cargo.toml"), "--offline"],
        cwd=root,
        check=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )

    if mode == "missing-catalog":
        (root / "Cargo.toml").write_text(
            '[workspace]\nresolver = "2"\nmembers = ["core", "app"]\n\n'
            "[workspace.dependencies]\n",
            encoding="utf-8",
        )
    elif mode == "wrong-version":
        (root / "Cargo.toml").write_text(
            '[workspace]\nresolver = "2"\nmembers = ["core", "app"]\n\n'
            '[workspace.dependencies]\ncore = { version = "0.1", path = "core" }\n',
            encoding="utf-8",
        )
    elif mode == "wrong-path":
        (root / "Cargo.toml").write_text(
            '[workspace]\nresolver = "2"\nmembers = ["core", "app"]\n\n'
            '[workspace.dependencies]\ncore = { version = "=0.1.0", path = "app" }\n',
            encoding="utf-8",
        )
    elif mode == "aliased-catalog":
        (root / "Cargo.toml").write_text(
            '[workspace]\nresolver = "2"\nmembers = ["core", "app"]\n\n'
            '[workspace.dependencies]\ncore = { version = "=0.1.0", path = "core" }\n'
            'core-alias = { package = "core", version = "=0.1.0", path = "core" }\n',
            encoding="utf-8",
        )
    elif mode in {"inline", "overridden", "target-inline"}:
        dependency = {
            "inline": 'core = { version = "=0.1.0", path = "../core" }',
            "overridden": 'core = { workspace = true, version = "=0.1.0" }',
            "target-inline": "",
        }[mode]
        app_manifest = (
            '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n'
            f"\n[dependencies]\n{dependency}\n"
        )
        if mode == "target-inline":
            app_manifest += (
                '\n[target.\'cfg(unix)\'.dependencies]\n'
                'core = { version = "=0.1.0", path = "../core" }\n'
            )
        (root / "app/Cargo.toml").write_text(app_manifest, encoding="utf-8")
    return root / "Cargo.toml", policy


def run_checker(manifest: Path, policy: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(CHECKER), str(manifest), "--policy", str(policy)],
        check=False,
        capture_output=True,
        text=True,
    )


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="panel-workspace-dependencies.") as temporary:
        root = Path(temporary)
        manifest, policy = create_fixture(root / "valid", "valid")
        result = run_checker(manifest, policy)
        if result.returncode != 0:
            raise AssertionError(f"valid catalog was rejected: {result.stderr}")

        scenarios = {
            "inline": "duplicated child dependency declaration",
            "overridden": "workspace dependency override",
            "target-inline": "target-specific duplicated declaration",
            "missing-catalog": "missing reusable member catalog entry",
            "wrong-version": "non-exact catalog version",
            "wrong-path": "redirected catalog path",
            "aliased-catalog": "duplicate aliased internal catalog entry",
        }
        for mode, scenario in scenarios.items():
            manifest, policy = create_fixture(root / mode, mode)
            if run_checker(manifest, policy).returncode == 0:
                raise AssertionError(f"{scenario} was not rejected")

        manifest, policy = create_fixture(root / "unknown-policy-field", "valid")
        document = json.loads(policy.read_text(encoding="utf-8"))
        document["members"]["core"]["unknown"] = True
        policy.write_text(json.dumps(document), encoding="utf-8")
        if run_checker(manifest, policy).returncode == 0:
            raise AssertionError("unknown boundary policy field was not rejected")

    print("Panel workspace dependency catalog self-test passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
