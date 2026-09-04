#!/usr/bin/env python3
"""Contract tests for the guard coverage meta-guard.

Each scenario starts from a tree that satisfies every property and removes
exactly one, so a passing run is evidence the guard reads that property rather
than evidence the fixture happens to be clean.
"""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

CHECKER = Path(__file__).resolve().with_name("check-guard-coverage.py")

WORKFLOW = """name: Fixture
on:
  push:
jobs:
  guards:
    steps:
      - run: python3 .github/scripts/test-example.py
      - run: python3 .github/scripts/check-example.py
      - run: bash .github/scripts/install-example.sh
      - run: bash .github/scripts/resolve-example.sh
      - run: python3 .github/scripts/emit-report.py
      - run: python3 .github/scripts/test-report.py
"""


def build(root: Path) -> tuple[Path, Path]:
    """Lay out a script directory and workflow that satisfy every property."""
    scripts = root / ".github" / "scripts"
    package = scripts / "policy"
    package.mkdir(parents=True)
    workflows = root / ".github" / "workflows"
    workflows.mkdir(parents=True)
    documents = root / ".github" / "policies"
    documents.mkdir(parents=True)
    (documents / "example.json").write_text('{"schema_version": 1}\n', encoding="utf-8")

    (package / "__init__.py").write_text(
        "from .errors import PolicyError\n\n__all__ = ['PolicyError']\n",
        encoding="utf-8",
    )
    (package / "errors.py").write_text(
        "class PolicyError(ValueError):\n    pass\n", encoding="utf-8"
    )
    (package / "shapes.py").write_text("VALUE = 1\n", encoding="utf-8")

    # Naming the document is what credits the guard with reading it.
    (scripts / "check-example.py").write_text(
        'from policy import shapes\n\nDOCUMENT = ".github/policies/example.json"\n',
        encoding="utf-8",
    )
    # The self-test names `shapes` directly and `errors` through the re-exported
    # PolicyError, which is what a test importing that name actually exercises.
    (scripts / "test-example.py").write_text(
        "from policy import PolicyError, shapes\n", encoding="utf-8"
    )
    (scripts / "emit-report.py").write_text("pass\n", encoding="utf-8")
    (scripts / "test-report.py").write_text("pass\n", encoding="utf-8")
    (scripts / "install-example.sh").write_text("#!/bin/sh\n", encoding="utf-8")
    (scripts / "resolve-example.sh").write_text("#!/bin/sh\n", encoding="utf-8")
    (scripts / "list-example.py").write_text("pass\n", encoding="utf-8")

    (workflows / "fixture.yml").write_text(WORKFLOW, encoding="utf-8")
    return scripts, workflows


def check(root: Path) -> int:
    return subprocess.run(
        [
            sys.executable,
            str(CHECKER),
            "--repo-root",
            str(root),
            "--script-root",
            str(root / ".github/scripts"),
            "--workflow-root",
            str(root / ".github/workflows"),
            "--policy-root",
            str(root / ".github/policies"),
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    ).returncode


def expect(actual: int, expected: int, scenario: str) -> None:
    if actual != expected:
        raise AssertionError(f"{scenario}: expected exit {expected}, got {actual}")


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="guard-coverage.") as temporary:
        base = Path(temporary)

        def scenario(name: str, expected: int, damage=None) -> None:
            root = base / name
            scripts, workflows = build(root)
            if damage is not None:
                damage(scripts, workflows)
            expect(check(root), expected, name)

        def wrap_example_guard(scripts: Path, workflows: Path, body: str) -> None:
            (scripts / "check-example.sh").write_text(body, encoding="utf-8")
            workflow = workflows / "fixture.yml"
            workflow.write_text(
                WORKFLOW.replace("check-example.py", "check-example.sh"),
                encoding="utf-8",
            )

        scenario("a tree satisfying every property", 0)
        scenario(
            "a shell guard delegating to its same-role implementation",
            0,
            lambda scripts, workflows: wrap_example_guard(
                scripts,
                workflows,
                '#!/bin/sh\nexec python3 "$repo_root/.github/scripts/check-example.py"\n',
            ),
        )
        scenario(
            "a shell guard only commenting on its implementation",
            1,
            lambda scripts, workflows: wrap_example_guard(
                scripts,
                workflows,
                "#!/bin/sh\n# check-example.py is not executed\nexit 0\n",
            ),
        )

        scenario(
            "a shared module no self-test names",
            1,
            lambda scripts, _: (scripts / "policy" / "orphan.py").write_text(
                "VALUE = 2\n", encoding="utf-8"
            ),
        )
        scenario(
            "a self-test naming a module that no longer exists",
            1,
            lambda scripts, _: (scripts / "policy" / "shapes.py").unlink(),
        )
        # `errors` is only ever reached through the re-exported PolicyError, so
        # losing the re-export map has to surface as uncovered rather than as a
        # module the guard forgot about.
        scenario(
            "a re-export the self-test no longer imports",
            1,
            lambda scripts, _: (scripts / "test-example.py").write_text(
                "from policy import shapes\n", encoding="utf-8"
            ),
        )
        scenario(
            "a guard with no paired self-test",
            1,
            lambda scripts, _: (scripts / "test-example.py").unlink(),
        )
        scenario(
            "a producer with no paired self-test",
            1,
            lambda scripts, _: (scripts / "test-report.py").unlink(),
        )
        scenario(
            "a guard no workflow invokes",
            1,
            lambda scripts, _: (scripts / "check-unrun.py").write_text(
                "from policy import shapes\n", encoding="utf-8"
            )
            or (scripts / "test-unrun.py").write_text("pass\n", encoding="utf-8"),
        )
        scenario(
            "a guard only mentioned outside executable run code",
            1,
            lambda _, workflows: (workflows / "fixture.yml").write_text(
                WORKFLOW.replace(
                    "      - run: python3 .github/scripts/check-example.py\n",
                    "      # check-example.py is documented but not invoked\n",
                )
                + "# .github/scripts/check-example.py\n",
                encoding="utf-8",
            ),
        )
        scenario(
            "a guard only echoed inside executable run code",
            1,
            lambda _, workflows: (workflows / "fixture.yml").write_text(
                WORKFLOW.replace(
                    "python3 .github/scripts/check-example.py",
                    "echo .github/scripts/check-example.py",
                ),
                encoding="utf-8",
            ),
        )
        scenario(
            "a guard launcher only echoed inside executable run code",
            1,
            lambda _, workflows: (workflows / "fixture.yml").write_text(
                WORKFLOW.replace(
                    "python3 .github/scripts/check-example.py",
                    "echo python3 .github/scripts/check-example.py",
                ),
                encoding="utf-8",
            ),
        )
        scenario(
            "a self-test no workflow invokes",
            1,
            lambda scripts, _: (scripts / "test-unrun.py").write_text(
                "pass\n", encoding="utf-8"
            ),
        )
        scenario(
            "a policy document no guard reads",
            1,
            lambda scripts, _: (
                scripts.parent / "policies" / "orphan.json"
            ).write_text('{"schema_version": 1}\n', encoding="utf-8"),
        )
        # A self-test naming a document does not make it enforced; only a guard
        # reading it does.
        scenario(
            "a policy document only a self-test mentions",
            1,
            lambda scripts, _: (
                (scripts.parent / "policies" / "mentioned.json").write_text(
                    '{"schema_version": 1}\n', encoding="utf-8"
                ),
                (scripts / "test-example.py").write_text(
                    'from policy import PolicyError, shapes\n'
                    '\nDOCUMENT = ".github/policies/mentioned.json"\n',
                    encoding="utf-8",
                ),
            ),
        )
        scenario(
            "a policy document only a guard comment mentions",
            1,
            lambda scripts, _: (
                (scripts.parent / "policies" / "commented.json").write_text(
                    '{"schema_version": 1}\n', encoding="utf-8"
                ),
                (scripts / "check-example.py").write_text(
                    "from policy import shapes\n"
                    '\nDOCUMENT = ".github/policies/example.json"\n'
                    "# commented.json is not read\n",
                    encoding="utf-8",
                ),
            ),
        )
        scenario(
            "a policy document only a guard docstring mentions",
            1,
            lambda scripts, _: (
                (scripts.parent / "policies" / "documented.json").write_text(
                    '{"schema_version": 1}\n', encoding="utf-8"
                ),
                (scripts / "check-example.py").write_text(
                    '\"\"\"documented.json is not read.\"\"\"\n'
                    "from policy import shapes\n"
                    '\nDOCUMENT = ".github/policies/example.json"\n',
                    encoding="utf-8",
                ),
            ),
        )
        scenario(
            "a script with no declared role",
            1,
            lambda scripts, _: (scripts / "helper-example.py").write_text(
                "pass\n", encoding="utf-8"
            ),
        )
        # A helper is declared as needing neither a test nor a workflow, so the
        # roles have to be read rather than applied uniformly.
        scenario(
            "a helper that is neither tested nor run",
            0,
            lambda scripts, _: (scripts / "list-second.py").write_text(
                "pass\n", encoding="utf-8"
            ),
        )
        # A shell self-test covers a Python guard: the pairing is about the
        # subject, not the language.
        scenario(
            "a guard paired with a shell self-test",
            0,
            lambda scripts, workflows: (
                (scripts / "test-example.py").unlink(),
                (scripts / "test-example.sh").write_text("#!/bin/sh\n", encoding="utf-8"),
                (scripts / "test-covers.py").write_text(
                    "from policy import PolicyError, shapes\n", encoding="utf-8"
                ),
                (workflows / "fixture.yml").write_text(
                    WORKFLOW.replace(
                        "test-example.py",
                        "test-example.sh\n      - run: python3 .github/scripts/test-covers.py",
                    ),
                    encoding="utf-8",
                ),
            ),
        )

        scenario(
            "a policy directory holding no documents",
            2,
            lambda scripts, _: (scripts.parent / "policies" / "example.json").unlink(),
        )
        scenario(
            "a policy directory that is missing",
            2,
            lambda scripts, _: shutil.rmtree(scripts.parent / "policies"),
        )
        scenario(
            "a shared package that is missing",
            2,
            lambda scripts, _: shutil.rmtree(scripts / "policy"),
        )
        scenario(
            "a self-test that does not parse",
            2,
            lambda scripts, _: (scripts / "test-example.py").write_text(
                "def broken(\n", encoding="utf-8"
            ),
        )
        scenario(
            "a script directory holding no scripts",
            2,
            lambda scripts, _: [
                path.unlink() for path in scripts.iterdir() if path.is_file()
            ],
        )

    print("Guard coverage self-test passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
