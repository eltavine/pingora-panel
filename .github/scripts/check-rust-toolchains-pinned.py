#!/usr/bin/env python3
"""Require exact Rust toolchains except explicitly marked canary lanes."""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

from policy import PolicyError, ci_yaml, registry


TOOLCHAIN_KEY = re.compile(
    r"^\s*(?:-\s*)?['\"]?(toolchain|rust-version|RUSTUP_TOOLCHAIN)['\"]?"
    r"\s*:\s*(.*?)\s*$"
)
FLOW_TOOLCHAIN_KEY = re.compile(
    r"(?:^|[{,]\s*)['\"]?(toolchain|rust-version|RUSTUP_TOOLCHAIN)['\"]?"
    r"\s*:\s*([^,}]+)"
)
SHELL_TOKEN = r"(?:'[^'\n]+'|\"[^\"\n]+\"|[^\s'\"\\]+)"
CARGO_FIRST_ARGUMENT = re.compile(r"\bcargo\s+(" + SHELL_TOKEN + r")")
RUSTUP_ENV_ASSIGNMENT = re.compile(
    r"(?:^|[\s;])RUSTUP_TOOLCHAIN\s*=\s*(" + SHELL_TOKEN + r")"
)
RUSTUP_POSITIONAL_TOOLCHAIN = re.compile(
    r"\brustup\s+(?:toolchain\s+(?:add|install)|install|update|default|override\s+set|run)\s+"
    r"(" + SHELL_TOKEN + r")"
)
RUSTUP_FLAG_TOOLCHAIN = re.compile(
    r"\brustup\s+[^#\n]*?--toolchain(?:=|\s+)(" + SHELL_TOKEN + r")"
)
MARKER = re.compile(r"#\s*rust-toolchain-policy:\s*([a-z0-9-]+)\s*$")
EXACT_VERSION = re.compile(r"\d+\.\d+\.\d+")
TOOLCHAIN_NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]*")
ALLOWANCE_MARKER = re.compile(r"[a-z0-9]+(?:-[a-z0-9]+)*")
MATRIX_EXPRESSION = "${{ matrix.toolchain }}"


@dataclass(frozen=True)
class Allowance:
    workflow: str
    value: str
    marker: str


def is_canonical_configuration_path(value: str) -> bool:
    path = PurePosixPath(value)
    if path.is_absolute() or path.as_posix() != value:
        return False
    parts = path.parts
    is_workflow = (
        len(parts) == 3
        and parts[:2] == (".github", "workflows")
        and path.suffix in {".yml", ".yaml"}
    )
    is_composite_action = (
        len(parts) >= 4
        and parts[:2] == (".github", "actions")
        and path.name in {"action.yml", "action.yaml"}
    )
    return is_workflow or is_composite_action


DOCUMENTS: registry.DocumentRegistry[
    None, tuple[frozenset[str], frozenset[Allowance]]
] = registry.DocumentRegistry("Rust toolchain policy")


@DOCUMENTS.reader(1)
def _read_v1(
    document: dict, _context: None
) -> tuple[frozenset[str], frozenset[Allowance]]:
    if set(document) != {"schema_version", "exact_versions", "floating_allowances"}:
        raise PolicyError(
            "Rust toolchain policy v1 must contain exactly schema_version, "
            "exact_versions, and floating_allowances"
        )
    versions = document.get("exact_versions")
    raw_allowances = document.get("floating_allowances")
    if (
        not isinstance(versions, list)
        or not versions
        or not all(isinstance(value, str) and EXACT_VERSION.fullmatch(value) for value in versions)
        or len(set(versions)) != len(versions)
        or not isinstance(raw_allowances, list)
    ):
        raise PolicyError("Rust toolchain policy has malformed exact versions")
    allowances: set[Allowance] = set()
    allowance_targets: set[tuple[str, str]] = set()
    for raw in raw_allowances:
        if not isinstance(raw, dict) or set(raw) != {"workflow", "value", "marker"}:
            raise PolicyError("Rust toolchain floating allowance is malformed")
        if not all(isinstance(raw[key], str) and raw[key] for key in raw):
            raise PolicyError("Rust toolchain floating allowance contains an empty value")
        workflow = raw["workflow"]
        if not is_canonical_configuration_path(workflow):
            raise PolicyError(
                "Rust toolchain allowance must name a canonical workflow or "
                f"composite-action YAML path: {workflow!r}"
            )
        if not TOOLCHAIN_NAME.fullmatch(raw["value"]):
            raise PolicyError(
                f"Rust toolchain allowance has malformed value: {raw['value']!r}"
            )
        if not ALLOWANCE_MARKER.fullmatch(raw["marker"]):
            raise PolicyError(
                f"Rust toolchain allowance has malformed marker: {raw['marker']!r}"
            )
        allowance = Allowance(raw["workflow"], raw["value"], raw["marker"])
        if allowance in allowances:
            raise PolicyError(f"duplicate Rust toolchain allowance: {allowance}")
        target = (allowance.workflow, allowance.value)
        if target in allowance_targets:
            raise PolicyError(
                "Rust toolchain allowance target has multiple markers: "
                f"{allowance.workflow} {allowance.value}"
            )
        allowances.add(allowance)
        allowance_targets.add(target)
    return frozenset(versions), frozenset(allowances)


def unquote(value: str) -> str:
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in {'"', "'"}:
        return value[1:-1]
    return value


def split_yaml_comment(line: str) -> tuple[str, str]:
    """Split one YAML comment without treating quoted or embedded hashes as comments."""

    single_quoted = False
    double_quoted = False
    escaped = False
    for index, character in enumerate(line):
        if escaped:
            escaped = False
            continue
        if character == "\\" and double_quoted:
            escaped = True
            continue
        if character == "'" and not double_quoted:
            single_quoted = not single_quoted
            continue
        if character == '"' and not single_quoted:
            double_quoted = not double_quoted
            continue
        if (
            character == "#"
            and not single_quoted
            and not double_quoted
            and (index == 0 or line[index - 1].isspace())
        ):
            return line[:index].rstrip(), line[index:]
    return line, ""


def logical_configuration_lines(text: str) -> list[tuple[int, str, str]]:
    """Join shell continuations while retaining the first physical line number."""

    logical: list[tuple[int, str, str]] = []
    buffered = ""
    start_line = 1
    for line_number, line in enumerate(text.splitlines(), 1):
        code, comment = split_yaml_comment(line)
        if not buffered:
            start_line = line_number
        stripped = code.rstrip()
        if stripped.endswith("\\"):
            buffered += stripped[:-1] + " "
            continue
        logical.append((start_line, buffered + code, comment))
        buffered = ""
    if buffered:
        logical.append((start_line, buffered, ""))
    return logical


def validate_value(
    *,
    value: str,
    marker: str | None,
    workflow: str,
    location: str,
    exact_versions: frozenset[str],
    allowances: frozenset[Allowance],
) -> str | None:
    value = unquote(value)
    if value == MATRIX_EXPRESSION:
        if marker is not None:
            return f"{location}: matrix expression must not carry a floating marker"
        return None
    if value.startswith("[") or value.endswith("]"):
        return f"{location}: inline toolchain collections are unsupported; use matrix include entries"
    if value in exact_versions:
        if marker is not None:
            return f"{location}: exact toolchain {value} must not carry a floating marker"
        return None
    if marker is None:
        return f"{location}: Rust toolchain {value!r} is not an approved exact version"
    allowance = Allowance(workflow, value, marker)
    if allowance not in allowances:
        return f"{location}: floating Rust toolchain allowance is not declared: {allowance}"
    return None


def shell_toolchain_candidates(code: str) -> list[str]:
    """Collect Rust toolchain selectors from executable shell source."""
    candidates: list[str] = []
    for argument in CARGO_FIRST_ARGUMENT.findall(code):
        argument = unquote(argument)
        if argument.startswith("+"):
            candidates.append(argument[1:])
    candidates.extend(RUSTUP_ENV_ASSIGNMENT.findall(code))
    candidates.extend(RUSTUP_POSITIONAL_TOOLCHAIN.findall(code))
    candidates.extend(RUSTUP_FLAG_TOOLCHAIN.findall(code))
    return candidates


def violations(
    repo_root: Path,
    workflow_root: Path,
    action_root: Path,
    exact_versions: frozenset[str],
    allowances: frozenset[Allowance],
) -> list[str]:
    if not workflow_root.is_dir():
        raise PolicyError(f"workflow directory does not exist: {workflow_root}")
    failures: list[str] = []
    used_allowances: set[Allowance] = set()
    workflows = sorted((*workflow_root.glob("*.yml"), *workflow_root.glob("*.yaml")))
    if not workflows:
        raise PolicyError(f"workflow directory contains no YAML files: {workflow_root}")
    if action_root.exists() and not action_root.is_dir():
        raise PolicyError(f"composite-action root is not a directory: {action_root}")
    composite_actions = (
        sorted((*action_root.rglob("action.yml"), *action_root.rglob("action.yaml")))
        if action_root.is_dir()
        else []
    )
    configurations = sorted(set((*workflows, *composite_actions)))
    for path in configurations:
        workflow = path.resolve().relative_to(repo_root).as_posix()
        text = path.read_text(encoding="utf-8")
        for line_number, code, comment in logical_configuration_lines(text):
            marker_match = MARKER.fullmatch(comment.strip())
            marker = marker_match.group(1) if marker_match else None
            key_match = TOOLCHAIN_KEY.match(code)
            candidates: list[str] = []
            if key_match:
                candidates.append(key_match.group(2).split(" #", 1)[0].strip())
            else:
                candidates.extend(
                    match.group(2).strip()
                    for match in FLOW_TOOLCHAIN_KEY.finditer(code)
                )
            for value in candidates:
                location = f"{workflow}:{line_number}"
                failure = validate_value(
                    value=value,
                    marker=marker,
                    workflow=workflow,
                    location=location,
                    exact_versions=exact_versions,
                    allowances=allowances,
                )
                if failure:
                    failures.append(failure)
                elif marker is not None:
                    used_allowances.add(Allowance(workflow, unquote(value), marker))
        # Shell selectors are scanned only from run blocks, using the same YAML
        # folded-scalar normalization as the installer and guard-coverage
        # policies. This prevents a `run: >` line break from hiding the token
        # following `cargo`, `rustup`, or `RUSTUP_TOOLCHAIN=`.
        for block_number, block in enumerate(ci_yaml.run_blocks(text), start=1):
            for line_number, code, comment in logical_configuration_lines(block):
                marker_match = MARKER.fullmatch(comment.strip())
                marker = marker_match.group(1) if marker_match else None
                for value in shell_toolchain_candidates(code):
                    location = f"{workflow}:run[{block_number}]:{line_number}"
                    failure = validate_value(
                        value=value,
                        marker=marker,
                        workflow=workflow,
                        location=location,
                        exact_versions=exact_versions,
                        allowances=allowances,
                    )
                    if failure:
                        failures.append(failure)
                    elif marker is not None:
                        used_allowances.add(Allowance(workflow, unquote(value), marker))
    unused = allowances - used_allowances
    failures.extend(f"unused Rust toolchain allowance: {allowance}" for allowance in sorted(
        unused, key=lambda item: (item.workflow, item.marker, item.value)
    ))
    return failures


def main(argv: list[str] | None = None) -> int:
    repo_root_default = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, default=repo_root_default)
    parser.add_argument("--workflow-root", type=Path)
    parser.add_argument("--action-root", type=Path)
    parser.add_argument(
        "--policy",
        type=Path,
        default=repo_root_default / ".github/policies/rust-toolchains.json",
    )
    arguments = parser.parse_args(argv)
    repo_root = arguments.repo_root.resolve()
    workflow_root = (
        arguments.workflow_root.resolve()
        if arguments.workflow_root
        else repo_root / ".github/workflows"
    )
    action_root = (
        arguments.action_root.resolve()
        if arguments.action_root
        else repo_root / ".github/actions"
    )
    try:
        exact_versions, allowances = DOCUMENTS.load(arguments.policy.resolve(), None)
        failures = violations(
            repo_root, workflow_root, action_root, exact_versions, allowances
        )
    except (OSError, UnicodeError, PolicyError, ValueError) as error:
        print(f"Rust toolchain policy failed closed: {error}", file=sys.stderr)
        return 2
    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print(
        "Rust workflow and composite-action toolchains satisfy the "
        "exact-version and canary policy."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
