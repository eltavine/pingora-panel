#!/usr/bin/env python3
"""Contract tests for the Dependabot ecosystem coverage guard.

Each fixture is a throwaway tree holding only the manifests and workflow
directory the guard discovers, so the assertions never depend on the real
repository layout.
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
from pathlib import Path

CHECKER = Path(__file__).resolve().with_name("check-dependabot-coverage.py")

WORKSPACE_MANIFEST = '[workspace]\nmembers = ["member"]\nresolver = "2"\n'
MEMBER_MANIFEST = '[package]\nname = "member"\nversion = "0.1.0"\nedition = "2021"\n'


def entry(ecosystem: str, directory: str) -> str:
    return (
        f"  - package-ecosystem: {ecosystem}\n"
        f"    directory: {directory}\n"
        "    schedule:\n"
        "      interval: weekly\n"
    )


def configuration(*entries: str) -> str:
    return "version: 2\nupdates:\n" + "\n".join(entries)


def build_tree(root: Path, workspaces: list[str]) -> None:
    (root / ".github/workflows").mkdir(parents=True)
    (root / ".github/workflows/build.yml").write_text("name: build\n", encoding="utf-8")
    for workspace in workspaces:
        directory = root if workspace == "/" else root / workspace.lstrip("/")
        (directory / "member/src").mkdir(parents=True, exist_ok=True)
        (directory / "Cargo.toml").write_text(WORKSPACE_MANIFEST, encoding="utf-8")
        (directory / "member/Cargo.toml").write_text(MEMBER_MANIFEST, encoding="utf-8")
        (directory / "member/src/lib.rs").write_text("", encoding="utf-8")


def check(root: Path, contents: str) -> int:
    path = root / ".github/dependabot.yml"
    path.write_text(contents, encoding="utf-8")
    return subprocess.run(
        [sys.executable, str(CHECKER), str(root), "--configuration", str(path)],
        check=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    ).returncode


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="dependabot-coverage.") as temporary:
        root = Path(temporary).resolve()
        build_tree(root, ["/", "/panel"])

        actions = entry("github-actions", "/")
        cargo_root = entry("cargo", "/")
        cargo_panel = entry("cargo", "/panel")

        def require(expected: int, scenario: str, contents: str) -> None:
            actual = check(root, contents)
            if actual != expected:
                raise AssertionError(f"{scenario}: expected exit {expected}, got {actual}")

        require(
            0,
            "every discovered ecosystem is covered",
            configuration(actions, cargo_root, cargo_panel),
        )
        require(
            0,
            "declaration order does not matter",
            configuration(cargo_panel, actions, cargo_root),
        )
        require(
            0,
            "quoted canonical keys are interpreted",
            configuration(actions, cargo_root, cargo_panel)
            .replace("version:", '"version":', 1)
            .replace("updates:", "'updates':", 1)
            .replace("package-ecosystem:", '"package-ecosystem":')
            .replace("directory:", "'directory':"),
        )
        require(
            1,
            "a workspace without a cargo entry",
            configuration(actions, cargo_root),
        )
        require(1, "no cargo entries at all", configuration(actions))
        require(
            1,
            "no github-actions entry",
            configuration(cargo_root, cargo_panel),
        )
        require(
            1,
            "a cargo entry for a directory that is not a workspace",
            configuration(actions, cargo_root, cargo_panel, entry("cargo", "/absent")),
        )
        require(
            1,
            "a github-actions entry outside the repository root",
            configuration(actions, entry("github-actions", "/panel"), cargo_root, cargo_panel),
        )
        require(
            1,
            "an ecosystem this guard does not govern",
            configuration(actions, cargo_root, cargo_panel, entry("npm", "/")),
        )
        require(
            2,
            "an entry that declares no directory",
            configuration(
                actions,
                cargo_root,
                cargo_panel,
                "  - package-ecosystem: cargo\n    schedule:\n      interval: weekly\n",
            ),
        )
        require(
            2,
            "the same ecosystem and directory declared twice",
            configuration(actions, cargo_root, cargo_panel, cargo_panel),
        )
        require(
            2,
            "an entry that does not begin with package-ecosystem",
            configuration(
                actions,
                cargo_root,
                cargo_panel,
                "  - directory: /panel\n    package-ecosystem: cargo\n",
            ),
        )
        require(2, "a configuration with no update entries", "version: 2\nupdates: []\n")
        require(
            2,
            "valid-looking entries outside updates",
            "version: 2\nupdates:\nelsewhere:\n" + actions + cargo_root + cargo_panel,
        )
        require(
            2,
            "a quoted duplicate directory overriding the governed value",
            configuration(
                actions,
                cargo_root,
                cargo_panel.replace(
                    "    schedule:\n", "    'directory': /absent\n    schedule:\n"
                ),
            ),
        )
        require(
            2,
            "a malformed quoted update field",
            configuration(actions, cargo_root, cargo_panel).replace(
                "    schedule:\n", '    "schedule:\n', 1
            ),
        )
        require(
            2,
            "duplicate top-level updates mappings",
            configuration(actions, cargo_root, cargo_panel) + "updates:\n",
        )
        require(
            2,
            "unsupported Dependabot schema version",
            configuration(actions, cargo_root, cargo_panel).replace("version: 2", "version: 3"),
        )

        # A commented-out entry must not count as coverage.
        require(
            1,
            "a commented-out cargo entry",
            configuration(actions, cargo_root)
            + "  # - package-ecosystem: cargo\n  #   directory: /panel\n",
        )

        # Nested workspaces are discovered too, so adding one fails until declared.
        nested = Path(temporary).resolve() / "nested"
        nested.mkdir()
        build_tree(nested, ["/", "/panel", "/tools/plugin"])
        if check(nested, configuration(actions, cargo_root, cargo_panel)) != 1:
            raise AssertionError("a newly added nested workspace was not required")
        if (
            check(
                nested,
                configuration(actions, cargo_root, cargo_panel, entry("cargo", "/tools/plugin")),
            )
            != 0
        ):
            raise AssertionError("a declared nested workspace was rejected")

        # A target directory is build output, never a governed workspace.
        ignored = Path(temporary).resolve() / "ignored"
        ignored.mkdir()
        build_tree(ignored, ["/", "/target/debug/vendored"])
        if check(ignored, configuration(actions, cargo_root)) != 0:
            raise AssertionError("a workspace under target/ was not ignored")

    print("Dependabot coverage policy self-test passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
