#!/usr/bin/env python3
"""Require every guard and every shared module to be tested, and both to run.

The guards in this directory are the only thing standing behind several policies
that have no other enforcement. That makes them load-bearing code whose own
failure mode is silence, and silence is indistinguishable from success. So the
guards are held to the properties they hold everything else to:

* every module in `policy/` is named by some self-test, because a regression in
  a shared primitive weakens every guard at once;
* every guard has a paired self-test, because a guard nobody tested may already
  be passing for the wrong reason;
* every guard and every self-test is invoked by a workflow, because one that is
  not run is not enforcement;
* every policy document is read by some guard, because a document nobody reads
  carries the authority of a policy while enforcing nothing, and reads in review
  as though the rule it describes is in force.

A module re-exported through `policy/__init__.py` is credited to the test that
imports the re-exported name, so `errors` counts as covered by a test importing
`PolicyError`. That is what the test actually exercises.

Extending this means adding a prefix to `ROLES`, not editing the logic.
"""

from __future__ import annotations

import ast
import re
from dataclasses import dataclass
from pathlib import Path

from policy import PolicyError, ci_yaml, cli

PACKAGE = "policy"
POLICY_DOCUMENTS = ".github/policies"

#: What each filename prefix is, and whether it must be paired and invoked.
#: A new class of script declares itself here rather than falling through as an
#: unclassified file the guard silently ignores.
ROLES: dict[str, tuple[str, bool, bool]] = {
    # prefix: (description, needs a paired self-test, must run in a workflow)
    "check-": ("guard", True, True),
    "emit-": ("producer", True, True),
    "test-": ("self-test", False, True),
    "install-": ("installer", False, True),
    "resolve-": ("baseline resolver", False, True),
    "list-": ("helper", False, False),
}

SCRIPT_SUFFIXES = frozenset({".py", ".sh"})
WORKFLOW_REFERENCE = re.compile(r"[A-Za-z0-9_.-]+\.(?:py|sh)")
SCRIPT_LAUNCHER = re.compile(r"(?:ba|d?a|z|k)?sh|python(?:3(?:\.\d+)?)?")


@dataclass(frozen=True)
class Script:
    path: Path
    role: str
    needs_test: bool
    needs_workflow: bool

    @property
    def stem(self) -> str:
        """The subject the script addresses, with role prefix and suffix removed.

        Neither is part of the subject, so `test-ci-tools-pinned.sh` pairs with
        `check-ci-tools-pinned.py`: a shell self-test covers a Python guard
        perfectly well, and requiring them to agree on a language would only
        push someone to satisfy the pairing rather than the guard.
        """
        for prefix in ROLES:
            if self.path.name.startswith(prefix):
                return self.path.stem[len(prefix) :]
        raise PolicyError(f"{self.path.name} has no known role prefix")


def classify(directory: Path) -> tuple[list[Script], list[str]]:
    """Sort scripts by role, reporting any file with no declared role."""
    scripts: list[Script] = []
    unclassified: list[str] = []
    for path in sorted(directory.iterdir()):
        if not path.is_file() or path.suffix not in SCRIPT_SUFFIXES:
            continue
        for prefix, (role, needs_test, needs_workflow) in ROLES.items():
            if path.name.startswith(prefix):
                scripts.append(Script(path, role, needs_test, needs_workflow))
                break
        else:
            unclassified.append(
                f"{path.name} matches no role in ROLES, so nothing decides whether "
                "it must be tested or run; give it a role or move it into "
                f"{PACKAGE}/"
            )
    if not scripts:
        raise PolicyError(f"no scripts found in {directory}")
    return scripts, unclassified


def reexports(package: Path) -> dict[str, str]:
    """Map each name `policy/__init__.py` re-exports to its defining module."""
    initialiser = package / "__init__.py"
    tree = parsed(initialiser)
    mapping: dict[str, str] = {}
    for node in ast.walk(tree):
        if isinstance(node, ast.ImportFrom) and node.level == 1 and node.module:
            for alias in node.names:
                mapping[alias.asname or alias.name] = node.module
    return mapping


def parsed(path: Path) -> ast.Module:
    try:
        return ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    except (OSError, UnicodeError, SyntaxError) as error:
        raise PolicyError(f"cannot parse {path}: {error}") from error


def named_modules(path: Path, reexported: dict[str, str]) -> set[str]:
    """Collect the `policy` modules a file names, directly or by re-export."""
    named: set[str] = set()
    for node in ast.walk(parsed(path)):
        if isinstance(node, ast.ImportFrom) and node.module:
            if node.module == PACKAGE:
                for alias in node.names:
                    if alias.name in reexported:
                        named.add(reexported[alias.name])
                    else:
                        named.add(alias.name)
            elif node.module.startswith(f"{PACKAGE}."):
                named.add(node.module.split(".", 2)[1])
        elif isinstance(node, ast.Import):
            for alias in node.names:
                if alias.name.startswith(f"{PACKAGE}."):
                    named.add(alias.name.split(".", 2)[1])
    return named


def python_semantic_strings(path: Path) -> str:
    """Return Python string literals except module/class/function docstrings."""
    tree = parsed(path)
    docstrings: set[int] = set()
    for node in ast.walk(tree):
        body = getattr(node, "body", None)
        if (
            isinstance(body, list)
            and body
            and isinstance(body[0], ast.Expr)
            and isinstance(body[0].value, ast.Constant)
            and isinstance(body[0].value.value, str)
        ):
            docstrings.add(id(body[0].value))
    return "\n".join(
        node.value
        for node in ast.walk(tree)
        if isinstance(node, ast.Constant)
        and isinstance(node.value, str)
        and id(node) not in docstrings
    )


def shell_semantic_source(path: Path) -> str:
    """Return active shell source with comments removed."""
    try:
        contents = path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as error:
        raise PolicyError(f"cannot read {path}: {error}") from error
    lines, _split_failures = ci_yaml.logical_lines(contents)
    return "\n".join(line.code for line in lines)


def document_readers(script_root: Path, documents: Path) -> list[str]:
    """Report any policy document no guard reads.

    A document is credited to a guard that names its filename. That is a
    textual test rather than a semantic one, which is the right strength here:
    the failure being caught is a document that has become detached from every
    reader, not one whose reader interprets it oddly.
    """
    if not documents.is_dir():
        raise PolicyError(f"policy document directory is missing: {documents}")
    present = sorted(path.name for path in documents.glob("*.json"))
    if not present:
        raise PolicyError(f"{documents} contains no policy documents")

    sources: dict[str, str] = {}
    for path in sorted(script_root.iterdir()):
        if not path.is_file() or not path.name.startswith("check-"):
            continue
        if path.suffix == ".py":
            sources[path.name] = python_semantic_strings(path)
        elif path.suffix == ".sh":
            sources[path.name] = shell_semantic_source(path)

    failures: list[str] = []
    for document in present:
        readers = sorted(
            name
            for name, text in sources.items()
            if document in text
        )
        if not readers:
            failures.append(
                f"{POLICY_DOCUMENTS}/{document} is read by no guard, so it reads "
                "as policy while enforcing nothing"
            )
    return failures


def executable_script_references(code: str) -> set[str]:
    """Return scripts used as commands, not merely mentioned in run text."""
    tokens = ci_yaml.shell_tokens(code)
    references: set[str] = set()
    for index, token in enumerate(tokens):
        launcher = Path(token).name
        previous = tokens[index - 1] if index else None
        command_position = index == 0 or previous in {
            "&",
            "&&",
            ";",
            "|",
            "||",
            "exec",
            "command",
            "env",
            "sudo",
        }
        if (
            command_position
            and SCRIPT_LAUNCHER.fullmatch(launcher) is not None
            and index + 1 < len(tokens)
        ):
            match = WORKFLOW_REFERENCE.search(tokens[index + 1])
            if match is not None:
                references.add(match.group(0))
        match = WORKFLOW_REFERENCE.search(token)
        if match is None:
            continue
        if index == 0 or previous in {"&", "&&", ";", "|", "||", "exec"}:
            references.add(match.group(0))
    for substitution in ci_yaml.shell_substitutions(code):
        references.update(executable_script_references(substitution))
    return references


def workflow_references(workflow_root: Path) -> set[str]:
    """Collect every script filename executed from a workflow ``run`` key."""
    referenced: set[str] = set()
    for path in sorted(workflow_root.rglob("*.yml")) + sorted(
        workflow_root.rglob("*.yaml")
    ):
        try:
            contents = path.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as error:
            raise PolicyError(f"cannot read {path}: {error}") from error
        for block in ci_yaml.run_blocks(contents):
            lines, _split_failures = ci_yaml.logical_lines(block)
            for line in lines:
                if WORKFLOW_REFERENCE.search(line.code) is not None:
                    referenced.update(executable_script_references(line.code))
    return referenced


def wrapper_references(path: Path) -> set[str]:
    """Collect active script references from one shell compatibility wrapper."""
    try:
        contents = path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as error:
        raise PolicyError(f"cannot read {path}: {error}") from error
    lines, _split_failures = ci_yaml.logical_lines(contents)
    return {
        reference
        for line in lines
        if WORKFLOW_REFERENCE.search(line.code) is not None
        for reference in executable_script_references(line.code)
    }


def invoked_scripts(scripts: list[Script], workflow_root: Path) -> set[str]:
    """Resolve direct calls plus same-role shell compatibility wrappers.

    A shell wrapper may remain as a stable entrypoint while delegating to a
    Python implementation. Credit that implementation only when the wrapper is
    itself executed, names it in active shell code, and shares its logical role
    and stem. Self-tests and unrelated launchers cannot satisfy enforcement.
    """
    invoked = workflow_references(workflow_root)
    by_name = {script.path.name: script for script in scripts}
    for launcher in scripts:
        if launcher.path.name not in invoked or launcher.path.suffix != ".sh":
            continue
        for reference in wrapper_references(launcher.path):
            dependency = by_name.get(reference)
            if (
                dependency is not None
                and dependency.role == launcher.role
                and dependency.stem == launcher.stem
            ):
                invoked.add(reference)
    return invoked


def violations(
    script_root: Path, workflow_root: Path, documents: Path
) -> list[str]:
    scripts, failures = classify(script_root)
    failures.extend(document_readers(script_root, documents))
    package = script_root / PACKAGE
    if not package.is_dir():
        raise PolicyError(f"shared policy package is missing: {package}")

    modules = {
        path.stem
        for path in sorted(package.glob("*.py"))
        if path.name != "__init__.py"
    }
    if not modules:
        raise PolicyError(f"{package} contains no modules to cover")

    reexported = reexports(package)
    tests = [script for script in scripts if script.role == "self-test"]
    covered: set[str] = set()
    for test in tests:
        if test.path.suffix == ".py":
            covered |= named_modules(test.path, reexported)

    failures.extend(
        f"{PACKAGE}/{module}.py is named by no self-test, so a regression in it "
        "would weaken every guard that depends on it unnoticed"
        for module in sorted(modules - covered)
    )
    # A test naming a module that no longer exists means the coverage claim has
    # gone stale and this guard would keep reporting it as satisfied.
    failures.extend(
        f"a self-test imports {PACKAGE}.{module}, which does not exist"
        for module in sorted(covered - modules - {"__init__"})
    )

    stems = {script.stem for script in tests}
    failures.extend(
        f"{script.path.name} is a {script.role} with no paired "
        f"test-{script.stem}, so nothing establishes that it can fail"
        for script in scripts
        if script.needs_test and script.stem not in stems
    )

    referenced = invoked_scripts(scripts, workflow_root)
    failures.extend(
        f"{script.path.name} is a {script.role} that no workflow invokes, so it "
        "is not enforcement"
        for script in scripts
        if script.needs_workflow and script.path.name not in referenced
    )
    return failures


def main(argv: list[str] | None = None) -> int:
    entry = cli.Entrypoint("Guard coverage", __doc__, dated=False)
    entry.add_repo_root()
    entry.parser.add_argument("--script-root", type=Path)
    entry.parser.add_argument("--workflow-root", type=Path)
    entry.parser.add_argument("--policy-root", type=Path)
    arguments = entry.parse(argv)

    try:
        repo_root = arguments.repo_root.resolve(strict=True)
        script_root = (
            arguments.script_root
            if arguments.script_root is not None
            else repo_root / ".github/scripts"
        ).resolve(strict=True)
        workflow_root = (
            arguments.workflow_root
            if arguments.workflow_root is not None
            else repo_root / ".github/workflows"
        ).resolve(strict=True)
        documents = (
            arguments.policy_root
            if arguments.policy_root is not None
            else repo_root / POLICY_DOCUMENTS
        )
        failures = violations(script_root, workflow_root, documents)
    except cli.FAILING as error:
        return entry.failed_closed(error)

    return entry.report(
        failures,
        "Every guard has a self-test, every shared module and policy document is "
        "covered, and every guard runs in CI",
        header="the guards are not held to the standard they enforce",
    )


if __name__ == "__main__":
    raise SystemExit(main())
