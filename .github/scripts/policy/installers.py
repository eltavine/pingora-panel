"""Immutability rules for the package installers CI is allowed to run.

Every rule answers the same three questions about one installer invocation:
does it name exactly one package, is that package pinned to one exact version,
and can the source be redirected away from the default registry?

Governing a new installer is one :data:`RULES` entry; the scanner and the guard
script never change.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from typing import Callable, Sequence

SHELL_OPERATORS = frozenset({"&", "&&", ";", "|", "||"})

EXACT_SEMANTIC_VERSION = re.compile(r"\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?")
CRATE_NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9_-]*")
GO_MODULE_AT_VERSION = re.compile(
    r"[A-Za-z0-9][A-Za-z0-9._~/-]*@v\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?"
)
# PEP 440 release segments plus the pre/post/dev suffixes real pins use.
PYTHON_REQUIREMENT = re.compile(
    r"[A-Za-z0-9][A-Za-z0-9._-]*"
    r"==\d+(?:\.\d+)*(?:(?:a|b|rc)\d+|\.post\d+|\.dev\d+)?"
)


@dataclass(frozen=True)
class OptionGrammar:
    """Which options an installer may carry, and which are outright forbidden.

    Forbidden options are split by arity so that rejecting one does not consume
    an unrelated argument: skipping the value of `--index-url` is correct, while
    skipping the token after a bare `--pre` would hide the requirement itself.
    """

    boolean: frozenset[str] = frozenset()
    valued: frozenset[str] = frozenset()
    forbidden_flags: frozenset[str] = frozenset()
    forbidden_valued: frozenset[str] = frozenset()


@dataclass(frozen=True)
class InstallerRule:
    """One governed installer invocation, e.g. ``cargo install``."""

    name: str
    programs: frozenset[str]
    subcommand: str
    check: Callable[[list[str]], list[str]]
    allows_toolchain_prefix: bool = False
    options: OptionGrammar = field(default_factory=OptionGrammar)

    def argument_start(self, tokens: Sequence[str], index: int) -> int | None:
        """Return where this rule's arguments begin at `index`, or None."""
        if tokens[index] not in self.programs:
            return None
        offset = (
            1
            if self.allows_toolchain_prefix
            and index + 1 < len(tokens)
            and tokens[index + 1].startswith("+")
            else 0
        )
        subcommand_index = index + 1 + offset
        if subcommand_index >= len(tokens) or tokens[subcommand_index] != self.subcommand:
            return None
        return subcommand_index + 1


def _partition_options(
    arguments: list[str], grammar: OptionGrammar, name: str
) -> tuple[list[str], dict[str, str], list[str]]:
    """Split arguments into positionals, option values, and policy failures."""
    positionals: list[str] = []
    values: dict[str, str] = {}
    failures: list[str] = []
    index = 0
    while index < len(arguments):
        argument = arguments[index]
        option, separator, inline = argument.partition("=")
        if option in grammar.forbidden_valued:
            failures.append(f"{name} source option is forbidden: {option}")
            if not separator:
                index += 1
        elif option in grammar.forbidden_flags:
            failures.append(f"{name} option is forbidden: {option}")
        elif option in grammar.boolean:
            if separator:
                failures.append(f"{name} flag must not take a value: {option}")
            elif option in values:
                failures.append(f"{name} repeats {option}")
            else:
                values[option] = ""
        elif option in grammar.valued:
            if option in values:
                failures.append(f"{name} repeats {option}")
            if separator:
                values[option] = inline
            elif index + 1 < len(arguments) and not arguments[index + 1].startswith("-"):
                index += 1
                values[option] = arguments[index]
            else:
                # Never absorb a token that looks like an option. Doing so lets a
                # forbidden one hide as an approved option's value, and an
                # installer whose values are not themselves validated would then
                # accept the redirect it exists to refuse.
                failures.append(f"{name} option requires a value: {option}")
                values[option] = ""
        elif argument.startswith("-"):
            failures.append(f"{name} option is not approved: {argument}")
        else:
            positionals.append(argument)
        index += 1
    return positionals, values, failures


CARGO_OPTIONS = OptionGrammar(
    boolean=frozenset({"--locked"}),
    valued=frozenset({"--version"}),
    forbidden_valued=frozenset(
        {"--git", "--path", "--registry", "--index", "--branch", "--tag", "--rev"}
    ),
)

PIP_OPTIONS = OptionGrammar(
    boolean=frozenset(
        {
            "--user",
            "--no-cache-dir",
            "--no-deps",
            "--no-input",
            "--disable-pip-version-check",
            "--require-hashes",
        }
    ),
    valued=frozenset({"--only-binary"}),
    forbidden_flags=frozenset({"--pre", "--no-index"}),
    forbidden_valued=frozenset(
        {
            "--index-url",
            "-i",
            "--extra-index-url",
            "--find-links",
            "-f",
            "-e",
            "--editable",
            "-r",
            "--requirement",
            "--trusted-host",
            "--target",
            "--proxy",
        }
    ),
)


def _check_cargo(arguments: list[str]) -> list[str]:
    name = "cargo install"
    packages, values, failures = _partition_options(arguments, CARGO_OPTIONS, name)
    if "--locked" not in values:
        failures.append(f"{name} must use --locked")
    version = values.get("--version")
    if version is None:
        failures.append(f"{name} must use one exact semantic --version")
    elif EXACT_SEMANTIC_VERSION.fullmatch(version) is None:
        failures.append(f"{name} must use one exact semantic --version")
    if len(packages) != 1 or CRATE_NAME.fullmatch(packages[0] if packages else "") is None:
        failures.append(f"{name} must name exactly one crates.io package")
    return failures


def _check_go(arguments: list[str]) -> list[str]:
    if len(arguments) != 1 or GO_MODULE_AT_VERSION.fullmatch(arguments[0]) is None:
        return ["go install must name exactly one module at an exact semantic version"]
    module = arguments[0].split("@", 1)[0]
    if "//" in module or any(part in {"", ".", ".."} for part in module.split("/")):
        return ["go install must name exactly one canonical module path"]
    return []


def _check_pip(arguments: list[str]) -> list[str]:
    name = "pip install"
    requirements, _values, failures = _partition_options(arguments, PIP_OPTIONS, name)
    if len(requirements) != 1:
        failures.append(f"{name} must name exactly one requirement")
    elif PYTHON_REQUIREMENT.fullmatch(requirements[0]) is None:
        failures.append(
            f"{name} must pin one requirement with == and an exact version: "
            f"{requirements[0]}"
        )
    return failures


CARGO_INSTALL = InstallerRule(
    name="cargo install",
    programs=frozenset({"cargo"}),
    subcommand="install",
    check=_check_cargo,
    allows_toolchain_prefix=True,
    options=CARGO_OPTIONS,
)

GO_INSTALL = InstallerRule(
    name="go install",
    programs=frozenset({"go"}),
    subcommand="install",
    check=_check_go,
)

PIP_INSTALL = InstallerRule(
    name="pip install",
    programs=frozenset({"pip", "pip3"}),
    subcommand="install",
    check=_check_pip,
    options=PIP_OPTIONS,
)

RULES: tuple[InstallerRule, ...] = (CARGO_INSTALL, GO_INSTALL, PIP_INSTALL)


def failures(tokens: list[str], rules: Sequence[InstallerRule] = RULES) -> list[str]:
    """Report every policy violation among the installer calls in one command."""
    reported: list[str] = []
    for index in range(len(tokens)):
        for rule in rules:
            start = rule.argument_start(tokens, index)
            if start is None:
                continue
            if any(token in SHELL_OPERATORS for token in tokens):
                reported.append(f"{rule.name} must be a standalone command")
                continue
            reported.extend(rule.check(list(tokens[start:])))
    return reported


def invocation_names(
    tokens: Sequence[str], rules: Sequence[InstallerRule] = RULES
) -> tuple[str, ...]:
    """Return governed installers invoked by one token stream."""
    return tuple(
        rule.name
        for index in range(len(tokens))
        for rule in rules
        if rule.argument_start(tokens, index) is not None
    )


__all__ = [
    "CARGO_INSTALL",
    "GO_INSTALL",
    "InstallerRule",
    "OptionGrammar",
    "PIP_INSTALL",
    "RULES",
    "SHELL_OPERATORS",
    "failures",
    "invocation_names",
]
