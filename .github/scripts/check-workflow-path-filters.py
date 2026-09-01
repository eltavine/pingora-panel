#!/usr/bin/env python3
"""Verify that path-filtered workflows cover their local executable dependencies."""

from __future__ import annotations

import argparse
import ast
import re
import sys
from dataclasses import dataclass
from pathlib import Path


LOCAL_REFERENCE = re.compile(r"\.github/scripts/[A-Za-z0-9_./-]+")
LOCAL_USE = re.compile(
    r"^\s*(?:-\s*)?uses:\s*['\"]?(\./[A-Za-z0-9_./-]+)", re.MULTILINE
)
EVENT = re.compile(r"([A-Za-z_][A-Za-z0-9_-]*):")
RUN = re.compile(r"^(\s*)run:\s*(.*)$")
BLOCK_SCALAR = re.compile(r"[|>](?:[1-9][+-]?|[+-][1-9]?)?")
PATH_FILTER_KEYS = ("paths", "paths-ignore")


@dataclass
class EventPathFilter:
    kind: str
    patterns: list[str]


def parse_scalar(value: str) -> str:
    value = value.strip()
    if value.startswith(("'", '"')):
        parsed = ast.literal_eval(value)
        if not isinstance(parsed, str):
            raise ValueError(f"path entry is not a string: {value}")
        return parsed
    return value.split(" #", 1)[0].strip()


def parse_path_filters(text: str) -> dict[str, EventPathFilter]:
    filters: dict[str, EventPathFilter] = {}
    inside_on = False
    current_event: str | None = None
    current_filter: EventPathFilter | None = None

    for raw_line in text.splitlines():
        stripped = raw_line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        indent = len(raw_line) - len(raw_line.lstrip())

        if indent == 0:
            if stripped.startswith("on:") and stripped != "on:" and "paths" in stripped:
                raise ValueError("inline on.paths syntax is unsupported; use block syntax")
            inside_on = stripped == "on:"
            current_event = None
            current_filter = None
            continue
        if not inside_on:
            continue
        if indent == 2:
            match = EVENT.fullmatch(stripped)
            if match is None and "paths" in stripped:
                raise ValueError("inline event paths syntax is unsupported; use block syntax")
            current_event = match.group(1) if match else None
            current_filter = None
            continue
        if indent == 4 and current_event is not None:
            filter_kind = next(
                (
                    key
                    for key in PATH_FILTER_KEYS
                    if stripped.startswith(f"{key}:")
                ),
                None,
            )
            if filter_kind is None:
                current_filter = None
                continue
            if stripped != f"{filter_kind}:":
                raise ValueError(
                    f"inline {filter_kind} syntax is unsupported; use a block list"
                )
            if current_event in filters:
                raise ValueError(
                    f"{current_event} defines multiple path filters; use exactly one"
                )
            current_filter = EventPathFilter(filter_kind, [])
            filters[current_event] = current_filter
            continue
        if current_filter is not None and indent >= 6 and stripped.startswith("- "):
            pattern = parse_scalar(stripped[2:])
            if not pattern:
                raise ValueError(f"{current_event}.{current_filter.kind} has an empty pattern")
            if current_filter.kind == "paths-ignore" and pattern.startswith("!"):
                raise ValueError("paths-ignore does not support negated patterns")
            current_filter.patterns.append(pattern)

    for event, path_filter in filters.items():
        if not path_filter.patterns:
            raise ValueError(f"{event}.{path_filter.kind} has no patterns")
        if path_filter.kind == "paths" and all(
            pattern.startswith("!") for pattern in path_filter.patterns
        ):
            raise ValueError(f"{event}.paths requires at least one positive pattern")

    return filters


def parse_run_blocks(text: str) -> str:
    lines = text.splitlines()
    commands: list[str] = []
    index = 0
    while index < len(lines):
        match = RUN.match(lines[index])
        if match is None:
            index += 1
            continue

        run_indent = len(match.group(1))
        value = match.group(2).strip()
        if BLOCK_SCALAR.fullmatch(value) is None:
            commands.append(value)
            index += 1
            continue

        index += 1
        block: list[str] = []
        while index < len(lines):
            line = lines[index]
            if line.strip():
                indent = len(line) - len(line.lstrip())
                if indent <= run_indent:
                    break
            block.append(line)
            index += 1
        commands.extend(block)
    return "\n".join(commands)


def glob_regex(pattern: str) -> re.Pattern[str]:
    tokens: list[str] = []
    index = 0
    while index < len(pattern):
        character = pattern[index]
        if character == "\\" and index + 1 < len(pattern):
            tokens.append(re.escape(pattern[index + 1]))
            index += 2
        elif pattern.startswith("**/", index) and (
            index == 0 or pattern[index - 1] == "/"
        ):
            # GitHub's **/ matches zero or more complete directory levels.
            tokens.append("(?:.*/)?")
            index += 3
        elif pattern.startswith("**", index):
            tokens.append(".*")
            index += 2
        elif character == "*":
            tokens.append("[^/]*")
            index += 1
        elif character == "[":
            closing = pattern.find("]", index + 1)
            content = pattern[index + 1 : closing] if closing >= 0 else ""
            if closing >= 0 and re.fullmatch(r"[A-Za-z0-9-]+", content):
                class_expression = f"[{content}]"
                try:
                    re.compile(class_expression)
                except re.error:
                    pass
                else:
                    tokens.append(class_expression)
                    index = closing + 1
                    continue
            tokens.append(re.escape(character))
            index += 1
        elif character in {"?", "+"} and tokens:
            previous = tokens.pop()
            quantifier = "?" if character == "?" else "+"
            tokens.append(f"(?:{previous}){quantifier}")
            index += 1
        else:
            tokens.append(re.escape(character))
            index += 1
    return re.compile("^" + "".join(tokens) + "$")


def path_is_included(path: str, patterns: list[str]) -> bool:
    included = False
    for raw_pattern in patterns:
        negative = raw_pattern.startswith("!")
        pattern = raw_pattern[1:] if negative else raw_pattern
        if glob_regex(pattern).fullmatch(path):
            included = not negative
    return included


def path_is_ignored(path: str, patterns: list[str]) -> bool:
    return any(glob_regex(pattern).fullmatch(path) for pattern in patterns)


def check_workflows(repo_root: Path, workflow_root: Path) -> list[str]:
    errors: list[str] = []
    workflows = sorted(
        path
        for path in workflow_root.iterdir()
        if path.is_file() and path.suffix in {".yml", ".yaml"}
    )
    for workflow in workflows:
        workflow_path = workflow.relative_to(repo_root).as_posix()
        text = workflow.read_text(encoding="utf-8")
        try:
            path_filters = parse_path_filters(text)
        except ValueError as error:
            errors.append(f"{workflow_path}: {error}")
            continue
        if not path_filters:
            continue

        references = set(LOCAL_REFERENCE.findall(parse_run_blocks(text)))
        references.update(
            match.group(1).removeprefix("./") for match in LOCAL_USE.finditer(text)
        )
        dependencies = {workflow_path}
        for reference in sorted(references):
            if ".." in Path(reference).parts:
                errors.append(
                    f"{workflow_path}: local dependency escapes repository: {reference}"
                )
                continue
            candidate = repo_root / reference
            resolved_candidate = candidate.resolve()
            try:
                resolved_candidate.relative_to(repo_root)
            except ValueError:
                errors.append(
                    f"{workflow_path}: local dependency escapes repository: {reference}"
                )
                continue
            if candidate.is_file():
                dependencies.add(reference)
                continue
            if candidate.is_dir():
                local_files = sorted(path for path in candidate.rglob("*") if path.is_file())
                if not local_files:
                    errors.append(
                        f"{workflow_path}: local dependency directory is empty: {reference}"
                    )
                    continue
                for local_file in local_files:
                    try:
                        local_file.resolve().relative_to(repo_root)
                    except ValueError:
                        errors.append(
                            f"{workflow_path}: local dependency escapes repository: "
                            f"{local_file.relative_to(repo_root).as_posix()}"
                        )
                        continue
                    dependencies.add(local_file.relative_to(repo_root).as_posix())
                continue
            errors.append(f"{workflow_path}: local dependency does not exist: {reference}")

        for dependency in sorted(dependencies):
            for event, path_filter in path_filters.items():
                if path_filter.kind == "paths":
                    covered = path_is_included(dependency, path_filter.patterns)
                    failure = "does not cover"
                else:
                    covered = not path_is_ignored(dependency, path_filter.patterns)
                    failure = "excludes"
                if not covered:
                    errors.append(
                        f"{workflow_path}: {event}.{path_filter.kind} "
                        f"{failure} {dependency}"
                    )
    return errors


def main() -> int:
    default_repo = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo-root", type=Path, default=default_repo)
    parser.add_argument("--workflow-root", type=Path)
    arguments = parser.parse_args()

    repo_root = arguments.repo_root.resolve()
    workflow_root = (
        arguments.workflow_root.resolve()
        if arguments.workflow_root
        else repo_root / ".github/workflows"
    )
    if not workflow_root.is_dir():
        parser.error(f"workflow directory does not exist: {workflow_root}")

    errors = check_workflows(repo_root, workflow_root)
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1
    print("Workflow path-filter coverage verified.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
