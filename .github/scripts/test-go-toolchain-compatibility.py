#!/usr/bin/env python3
"""Contract tests for the Go toolchain compatibility guard."""

from __future__ import annotations

import importlib.util
import os
import subprocess
import sys
import tempfile
from pathlib import Path


SCRIPT_DIRECTORY = Path(__file__).resolve().parent
CHECKER = SCRIPT_DIRECTORY / "check-go-toolchain-compatibility.py"
SPEC = importlib.util.spec_from_file_location("go_compatibility", CHECKER)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"cannot load {CHECKER}")
MODULE = importlib.util.module_from_spec(SPEC)
sys.dont_write_bytecode = True
SPEC.loader.exec_module(MODULE)


def write_fake_go(directory: Path) -> Path:
    fake = directory / "fake-go"
    fake.write_text(
        """#!/usr/bin/env python3
import json
import os
import sys

if sys.argv[1:] == ["env", "GOVERSION"]:
    print(os.environ["FAKE_GO_CURRENT"])
elif sys.argv[1:3] == ["list", "-m"] and sys.argv[3] == "-json":
    required = os.environ["FAKE_GO_REQUIRED"]
    print(json.dumps({"GoVersion": required} if required else {}))
else:
    raise SystemExit(2)
""",
        encoding="utf-8",
    )
    fake.chmod(0o755)
    return fake


def run_guard(fake: Path, current: str, required: str | None, module: str) -> int:
    """Run the guard against a stand-in `go`. `required=None` omits GoVersion."""
    environment = os.environ.copy()
    environment.update(
        FAKE_GO_CURRENT=current,
        FAKE_GO_REQUIRED=required or "",
    )
    return subprocess.run(
        [
            sys.executable,
            str(CHECKER),
            "--go-command",
            str(fake),
            module,
        ],
        env=environment,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    ).returncode


def expect(actual: object, expected: object, scenario: str) -> None:
    if actual != expected:
        raise AssertionError(f"{scenario}: expected {expected!r}, got {actual!r}")


def main() -> int:
    expect(MODULE.parse_go_version("go1.25.13"), (1, 25, 13), "release version")
    expect(MODULE.parse_go_version("1.25"), (1, 25, 0), "module version")

    with tempfile.TemporaryDirectory(prefix="go-toolchain-compatibility.") as root:
        fake = write_fake_go(Path(root))
        exact_module = "example.com/tool@v1.7.12"
        expect(
            run_guard(fake, "go1.25.13", "1.25.0", exact_module),
            0,
            "newer patch release",
        )
        expect(
            run_guard(fake, "go1.25.0", "1.25.0", exact_module),
            0,
            "equal minimum release",
        )
        expect(
            run_guard(fake, "go1.24.13", "1.25.0", exact_module),
            1,
            "older toolchain",
        )
        expect(
            run_guard(fake, "go1.25rc1", "1.25.0", exact_module),
            1,
            "prerelease toolchain",
        )
        expect(
            run_guard(fake, "go1.25.13", "1.25.0", "example.com/tool@latest"),
            1,
            "floating module version",
        )

        # Exit 2 is reserved for never reaching a verdict, and has to be
        # reachable, or the guard's own docstring is untested.
        expect(
            run_guard(Path(root) / "absent-go", "go1.25.13", "1.25.0", exact_module),
            2,
            "no Go toolchain on PATH",
        )
        expect(
            run_guard(fake, "go1.25.13", "not-a-version", exact_module),
            2,
            "module metadata declaring an unreadable Go version",
        )
        expect(
            run_guard(fake, "go1.25.13", None, exact_module),
            2,
            "module metadata declaring no Go version",
        )

    print("Go toolchain compatibility guard self-test passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
