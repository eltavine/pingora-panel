#!/usr/bin/env python3
"""Fail closed when the active Go toolchain cannot build a pinned module.

Two outcomes are kept apart. A toolchain older than the module requires is a
policy violation, reported as exit 1. Being unable to *establish* either version
— no Go on PATH, unreadable module metadata — is not a verdict, and is reported
as exit 2. Conflating them would let an environment failure read as either a
clean run or a routine violation, and neither is true.
"""

from __future__ import annotations

import json
import re
import subprocess
from pathlib import Path

from policy import PolicyError, cli


EXACT_MODULE = re.compile(
    r"^[^@\s]+@v\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$"
)
GO_VERSION = re.compile(r"^(?:go)?(\d+)\.(\d+)(?:\.(\d+))?$")


def parse_go_version(value: str) -> tuple[int, int, int]:
    match = GO_VERSION.fullmatch(value.strip())
    if match is None:
        raise PolicyError(f"invalid Go version: {value!r}")
    major, minor, patch = match.groups()
    return int(major), int(minor), int(patch or 0)


def incompatibilities(current: str, minimum: str) -> list[str]:
    """Report why the active toolchain cannot build the module, if it cannot.

    The two version strings fail differently on purpose. Unreadable *module*
    metadata means the guard never learned what the module needs, so it raises
    and the caller fails closed. An active toolchain that does not name a
    release — a release candidate, say — is instead a verdict: it may not build
    a module that requires one, and saying so is the guard working, not failing.
    """
    required = parse_go_version(minimum)
    try:
        active = parse_go_version(current)
    except PolicyError as error:
        return [str(error)]
    if active < required:
        return [f"Go {current.removeprefix('go')} is older than required Go {minimum}"]
    return []


def run_go(go_command: Path, *arguments: str) -> str:
    try:
        completed = subprocess.run(
            [str(go_command), *arguments],
            check=True,
            capture_output=True,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        detail = getattr(error, "stderr", "") or str(error)
        raise PolicyError(
            f"Go command failed for {' '.join(arguments)}: {detail.strip()}"
        ) from error
    return completed.stdout.strip()


def unpinned(module: str) -> list[str]:
    """Report a module argument that does not name exactly one release."""
    if EXACT_MODULE.fullmatch(module) is None:
        return [
            "module must use one exact semantic version such as example.com/tool@v1.2.3"
        ]
    return []


def module_go_version(go_command: Path, module: str) -> str:
    """Read a module's declared Go requirement. `module` must already be pinned."""
    try:
        metadata = json.loads(run_go(go_command, "list", "-m", "-json", module))
    except json.JSONDecodeError as error:
        raise PolicyError("go list returned invalid JSON metadata") from error
    required = metadata.get("GoVersion")
    if not isinstance(required, str) or not required:
        raise PolicyError(f"module metadata has no GoVersion: {module}")
    return required


def main(argv: list[str] | None = None) -> int:
    entry = cli.Entrypoint("Go toolchain compatibility", __doc__, dated=False)
    entry.parser.add_argument("module", help="exact module version to inspect")
    entry.parser.add_argument(
        "--go-command",
        type=Path,
        default=Path("go"),
        help="Go executable used for version and module metadata queries",
    )
    arguments = entry.parse(argv)

    # Validated before touching the environment, so an unpinned request is
    # answered without a network round trip deciding it for us.
    unpinned_module = unpinned(arguments.module)
    if unpinned_module:
        return entry.report(unpinned_module, "")

    try:
        current = run_go(arguments.go_command, "env", "GOVERSION")
        minimum = module_go_version(arguments.go_command, arguments.module)
        failures = incompatibilities(current, minimum)
    except cli.FAILING as error:
        return entry.failed_closed(error)

    return entry.report(
        failures,
        f"Go toolchain compatibility verified: {current} satisfies "
        f"{arguments.module} (Go >= {minimum})",
    )


if __name__ == "__main__":
    raise SystemExit(main())
