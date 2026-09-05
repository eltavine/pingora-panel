#!/usr/bin/env python3
"""Hermetic contract tests for the upstream security lockfile resolver."""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
from pathlib import Path


SOURCE = Path(__file__).resolve().with_name("resolve-security-lockfile.sh")


def executable(path: Path, contents: str) -> None:
    path.write_text(contents, encoding="utf-8")
    path.chmod(0o755)


def fixture(root: Path) -> tuple[Path, Path, Path]:
    scripts = root / ".github" / "scripts"
    scripts.mkdir(parents=True)
    resolver = scripts / SOURCE.name
    shutil.copy2(SOURCE, resolver)
    (root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")

    commands = root / "commands"
    commands.mkdir()
    log = root / "cargo.log"
    executable(
        commands / "cargo",
        """#!/bin/sh
set -eu
printf '%s\\n' "$*" >> "$FAKE_CARGO_LOG"
if [ "$1" = generate-lockfile ]; then
    if [ "${FAKE_CARGO_FAIL_GENERATE:-0}" = 1 ]; then
        exit 23
    fi
    if [ "${FAKE_LOCK_HAS_TIME:-0}" = 1 ]; then
        printf 'name = "time"\\nversion = "0.3.45"\\n' > Cargo.lock
    else
        printf 'name = "unrelated"\\nversion = "1.0.0"\\n' > Cargo.lock
    fi
fi
""",
    )
    executable(
        commands / "rg",
        """#!/bin/sh
set -eu
if [ "${FAKE_LOCK_HAS_TIME:-0}" = 1 ]; then
    exit 0
fi
exit 1
""",
    )
    return resolver, commands, log


def run(root: Path, arguments: list[str], **overrides: str) -> subprocess.CompletedProcess[str]:
    resolver, commands, log = fixture(root)
    environment = os.environ.copy()
    environment.update(overrides)
    environment["FAKE_CARGO_LOG"] = str(log)
    environment["PATH"] = f"{commands}{os.pathsep}{environment.get('PATH', '')}"
    return subprocess.run(
        ["/bin/bash", str(resolver), *arguments],
        cwd=root,
        env=environment,
        check=False,
        text=True,
        capture_output=True,
    )


def cargo_calls(root: Path) -> list[str]:
    log = root / "cargo.log"
    return log.read_text(encoding="utf-8").splitlines() if log.exists() else []


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="security-lockfile-resolver.") as temporary:
        base = Path(temporary)

        with_time = base / "with-time"
        result = run(with_time, [], FAKE_LOCK_HAS_TIME="1")
        if result.returncode != 0:
            raise AssertionError(f"time resolution failed: {result.stderr}")
        if cargo_calls(with_time) != [
            "generate-lockfile --manifest-path Cargo.toml --ignore-rust-version",
            "update --manifest-path Cargo.toml -p time --precise 0.3.47 --ignore-rust-version",
        ]:
            raise AssertionError("patched time resolution did not use the exact Cargo contract")

        without_time = base / "without-time"
        result = run(without_time, [], FAKE_LOCK_HAS_TIME="0")
        if result.returncode != 0:
            raise AssertionError(f"time-free resolution failed: {result.stderr}")
        if cargo_calls(without_time) != [
            "generate-lockfile --manifest-path Cargo.toml --ignore-rust-version"
        ]:
            raise AssertionError("a time-free graph unexpectedly ran cargo update")

        invalid = base / "invalid-manifest"
        result = run(invalid, ["panel/Cargo.toml"], FAKE_LOCK_HAS_TIME="1")
        if result.returncode != 2 or cargo_calls(invalid):
            raise AssertionError("a non-upstream manifest was not rejected before Cargo ran")

        failure = base / "generation-failure"
        result = run(
            failure,
            [],
            FAKE_LOCK_HAS_TIME="1",
            FAKE_CARGO_FAIL_GENERATE="1",
        )
        if result.returncode != 23 or len(cargo_calls(failure)) != 1:
            raise AssertionError("Cargo generation failure was not propagated fail-closed")

    print("Security lockfile resolver self-test passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
