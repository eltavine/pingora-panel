#!/usr/bin/env python3
"""Enforce least privilege, bounded jobs, and explicit workflow concurrency.

Per-job privilege escalations are the strongest exceptions this repository
grants — `issues: write` on a workflow that otherwise only reads. They are held
to the same lease contract as every other exception, so an escalation names a
registered owner, says why, and comes up for review on a date. A permission
grant is the last thing that should outlive the reason for it.

The document is read through a versioned registry, so a future schema is added
by registering a reader beside this one rather than by editing it.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date
from pathlib import Path, PurePosixPath

from policy import PolicyError, ci_yaml, cli, leases, registry


PERMISSION_NAME = re.compile(r"[a-z][a-z0-9-]*")
PERMISSION_LEVELS = frozenset({"none", "read", "write"})
JOB_NAME = re.compile(r"[A-Za-z0-9_-]+")


def key_pattern(name: str) -> str:
    """Return a YAML spelling for one simple key, including quoted forms."""
    escaped = re.escape(name)
    return rf'(?:{escaped}|"{escaped}"|\'{escaped}\')'


SIMPLE_PERMISSION_KEY = (
    r'(?:(?P<double>"[a-z][a-z0-9-]*")|'
    r"(?P<single>'[a-z][a-z0-9-]*')|(?P<plain>[a-z][a-z0-9-]*))"
)
SIMPLE_JOB_KEY = (
    r'(?:(?P<double>"[A-Za-z0-9_-]+")|'
    r"(?P<single>'[A-Za-z0-9_-]+')|(?P<plain>[A-Za-z0-9_-]+))"
)


def matched_key(match: re.Match[str]) -> str:
    """Return the canonical value captured by a simple-key expression."""
    value = match.group("double") or match.group("single") or match.group("plain")
    return value[1:-1] if value[:1] in {'"', "'"} else value


def scalar(value: str, context: str) -> str:
    """Read a plain or simply quoted scalar without lossy quote stripping."""
    value = value.strip()
    if not value:
        raise PolicyError(f"{context} must not be empty")
    if value[0] in {'"', "'"}:
        if len(value) < 2 or value[-1] != value[0]:
            raise PolicyError(f"{context} has mismatched quotes")
        return value[1:-1]
    if value[-1] in {'"', "'"}:
        raise PolicyError(f"{context} has mismatched quotes")
    return value


def canonical_workflow_path(value: str) -> bool:
    path = PurePosixPath(value)
    return (
        path.as_posix() == value
        and len(path.parts) == 3
        and path.parts[:2] == (".github", "workflows")
        and path.suffix in {".yml", ".yaml"}
    )


def validate_permissions(value: object, context: str) -> dict[str, str]:
    if not isinstance(value, dict) or not value:
        raise PolicyError(f"{context} permissions must be a non-empty mapping")
    permissions: dict[str, str] = {}
    for name, level in value.items():
        if (
            not isinstance(name, str)
            or PERMISSION_NAME.fullmatch(name) is None
            or not isinstance(level, str)
            or level not in PERMISSION_LEVELS
        ):
            raise PolicyError(f"{context} contains a malformed permission")
        permissions[name] = level
    return permissions


def validate_job_permissions(
    value: object, workflow: str, today: date
) -> dict[str, dict[str, str]]:
    """Read one workflow's leased per-job privilege escalations."""
    if not isinstance(value, dict) or not value:
        raise PolicyError(f"{workflow} job_permissions must be a non-empty mapping")
    escalations: dict[str, dict[str, str]] = {}
    for job, entry in value.items():
        context = f"{workflow} job {job}"
        if not isinstance(job, str) or JOB_NAME.fullmatch(job) is None:
            raise PolicyError(f"{workflow} job_permissions names a malformed job: {job!r}")
        mapping, _ = leases.read_lease(entry, frozenset({"permissions"}), context, today)
        escalations[job] = validate_permissions(mapping["permissions"], context)
    return escalations


@dataclass(frozen=True)
class WorkflowContract:
    """What one workflow must declare, including any leased escalations."""

    permissions: dict[str, str]
    cancel_in_progress: bool
    job_permissions: dict[str, dict[str, str]] = field(default_factory=dict)


@dataclass(frozen=True)
class ExecutionPolicy:
    """The whole workflow execution contract for the repository."""

    max_job_timeout_minutes: int
    workflows: dict[str, WorkflowContract]


DOCUMENTS: registry.DocumentRegistry[date, ExecutionPolicy] = registry.DocumentRegistry(
    "workflow execution policy"
)


@DOCUMENTS.reader(1)
def _read_v1(document: dict, today: date) -> ExecutionPolicy:
    if set(document) != {"schema_version", "max_job_timeout_minutes", "workflows"}:
        raise PolicyError(
            "workflow execution policy v1 must contain exactly schema_version, "
            "max_job_timeout_minutes, and workflows"
        )
    maximum = document["max_job_timeout_minutes"]
    workflows = document["workflows"]
    if (
        not isinstance(maximum, int)
        or isinstance(maximum, bool)
        or not 1 <= maximum <= 360
        or not isinstance(workflows, dict)
        or not workflows
    ):
        raise PolicyError("workflow execution policy has malformed global limits")
    contracts: dict[str, WorkflowContract] = {}
    for workflow, entry in workflows.items():
        if (
            not isinstance(workflow, str)
            or not canonical_workflow_path(workflow)
            or not isinstance(entry, dict)
            or not set(entry)
            <= {"permissions", "cancel_in_progress", "job_permissions"}
            or not {"permissions", "cancel_in_progress"} <= set(entry)
            or not isinstance(entry.get("cancel_in_progress"), bool)
        ):
            raise PolicyError(f"malformed workflow execution entry: {workflow!r}")
        contracts[workflow] = WorkflowContract(
            permissions=validate_permissions(entry["permissions"], workflow),
            cancel_in_progress=entry["cancel_in_progress"],
            job_permissions=(
                validate_job_permissions(entry["job_permissions"], workflow, today)
                if "job_permissions" in entry
                else {}
            ),
        )
    return ExecutionPolicy(max_job_timeout_minutes=maximum, workflows=contracts)


def unique_root_block(lines: list[str], name: str) -> tuple[int, int]:
    declarations = [
        (index, match.group(1))
        for index, line in enumerate(lines)
        if (match := re.fullmatch(rf"{key_pattern(name)}:(.*)", line)) is not None
    ]
    if len(declarations) != 1:
        raise PolicyError(f"workflow must define exactly one top-level {name} mapping")
    start, value = declarations[0]
    if value.strip():
        raise PolicyError(f"top-level {name} must be a block mapping")
    end = next(
        (
            index
            for index in range(start + 1, len(lines))
            if lines[index] and not lines[index][0].isspace()
        ),
        len(lines),
    )
    return start, end


def parse_string_mapping(lines: list[str], name: str) -> dict[str, str]:
    start, end = unique_root_block(lines, name)
    mapping: dict[str, str] = {}
    for line in lines[start + 1 : end]:
        if not line.strip():
            continue
        match = re.fullmatch(rf"  {SIMPLE_PERMISSION_KEY}:\s*(.*?)\s*", line)
        if match is None:
            raise PolicyError(f"top-level {name} must be a simple two-space mapping")
        key = matched_key(match)
        if key in mapping:
            raise PolicyError(f"top-level {name} repeats {key}")
        mapping[key] = scalar(match.group(4), f"top-level {name}.{key}")
    if not mapping:
        raise PolicyError(f"top-level {name} mapping must not be empty")
    return mapping


def parse_job_permissions(block: list[str], start: int) -> dict[str, str]:
    """Read one four-space job `permissions` mapping into an exact level map."""
    permissions: dict[str, str] = {}
    for line in block[start + 1 :]:
        if not line.strip():
            continue
        if len(line) - len(line.lstrip()) <= 4:
            break
        match = re.fullmatch(rf"      {SIMPLE_PERMISSION_KEY}:\s*(.*?)\s*", line)
        if match is None:
            raise PolicyError(
                "job permissions must be a simple six-space mapping of named levels"
            )
        key = matched_key(match)
        if key in permissions:
            raise PolicyError(f"job permissions repeat {key}")
        permissions[key] = scalar(match.group(4), f"job permission {key}")
    if not permissions:
        raise PolicyError("job permissions mapping must not be empty")
    return permissions


def parse_jobs(
    lines: list[str], maximum: int, escalations: dict[str, dict[str, str]]
) -> list[str]:
    start, end = unique_root_block(lines, "jobs")
    job_starts: list[tuple[int, str]] = []
    for index in range(start + 1, end):
        line = lines[index]
        if not line.strip() or len(line) - len(line.lstrip()) != 2:
            continue
        match = re.fullmatch(rf"  {SIMPLE_JOB_KEY}:\s*", line)
        if match is None:
            raise PolicyError("workflow jobs must use simple block-mapping job identifiers")
        job_starts.append((index, matched_key(match)))
    if not job_starts:
        raise PolicyError("workflow jobs mapping must contain at least one job")
    names = [name for _, name in job_starts]
    duplicates = sorted({name for name in names if names.count(name) > 1})
    if duplicates:
        raise PolicyError(f"workflow jobs mapping repeats job identifiers: {duplicates}")

    failures: list[str] = []
    overriding: set[str] = set()
    for position, (job_start, job_name) in enumerate(job_starts):
        job_end = (
            job_starts[position + 1][0]
            if position + 1 < len(job_starts)
            else end
        )
        block = lines[job_start + 1 : job_end]
        timeouts = [
            int(match.group(1))
            for line in block
            if (
                match := re.fullmatch(
                    rf"    {key_pattern('timeout-minutes')}:\s*([0-9]+)\s*", line
                )
            )
            is not None
        ]
        if len(timeouts) != 1:
            failures.append(f"job {job_name} must define exactly one timeout-minutes")
        elif not 1 <= timeouts[0] <= maximum:
            failures.append(
                f"job {job_name} timeout {timeouts[0]} exceeds the 1..{maximum} minute policy"
            )
        declarations = [
            (index, match.group(1).strip())
            for index, line in enumerate(block)
            if (
                match := re.fullmatch(
                    rf"    {key_pattern('permissions')}:(.*)", line
                )
            )
            is not None
        ]
        starts = [index for index, value in declarations if not value]
        if declarations:
            overriding.add(job_name)
            if job_name not in escalations:
                failures.append(
                    f"job {job_name} must not override top-level workflow permissions"
                )
        if len(declarations) != len(starts):
            failures.append(
                f"job {job_name} must declare permissions as a block mapping, not a scalar"
            )
        if len(starts) > 1:
            failures.append(f"job {job_name} declares multiple permissions mappings")
        elif len(starts) == 1 and job_name in escalations:
            try:
                declared = parse_job_permissions(block, starts[0])
            except PolicyError as error:
                failures.append(f"job {job_name}: {error}")
            else:
                if declared != escalations[job_name]:
                    failures.append(
                        f"job {job_name} permissions differ from policy "
                        f"(expected={escalations[job_name]}, actual={declared})"
                    )

    unused = sorted(set(escalations) - overriding)
    if unused:
        failures.append(
            f"policy grants permissions to jobs that do not override them: {unused}"
        )
    return failures


def workflow_failures(
    path: Path, expected: WorkflowContract, maximum: int
) -> list[str]:
    try:
        lines = [
            ci_yaml.strip_comment(line).rstrip()
            for line in path.read_text(encoding="utf-8").splitlines()
        ]
    except (OSError, UnicodeError) as error:
        raise PolicyError(f"cannot read workflow {path}: {error}") from error
    failures: list[str] = []
    try:
        actual_permissions = parse_string_mapping(lines, "permissions")
        if actual_permissions != expected.permissions:
            failures.append(
                "top-level permissions differ from policy "
                f"(expected={expected.permissions}, actual={actual_permissions})"
            )
        concurrency = parse_string_mapping(lines, "concurrency")
        if not concurrency.get("group"):
            failures.append("concurrency.group must be non-empty")
        expected_cancel = str(expected.cancel_in_progress).lower()
        if concurrency.get("cancel-in-progress") != expected_cancel:
            failures.append(
                "concurrency.cancel-in-progress differs from policy "
                f"(expected={expected_cancel!r})"
            )
        if set(concurrency) != {"group", "cancel-in-progress"}:
            failures.append(
                "concurrency mapping must contain exactly group and cancel-in-progress"
            )
        failures.extend(parse_jobs(lines, maximum, expected.job_permissions))
    except PolicyError as error:
        failures.append(str(error))
    return failures


def violations(
    repo_root: Path, workflow_root: Path, policy_path: Path, today: date
) -> list[str]:
    policy = DOCUMENTS.load(policy_path, today)
    maximum = policy.max_job_timeout_minutes
    contracts = policy.workflows
    if not workflow_root.is_dir():
        raise PolicyError(f"workflow directory does not exist: {workflow_root}")
    workflows = sorted((*workflow_root.glob("*.yml"), *workflow_root.glob("*.yaml")))
    if not workflows:
        raise PolicyError(f"workflow directory contains no YAML files: {workflow_root}")
    actual = {path.resolve().relative_to(repo_root).as_posix() for path in workflows}
    expected = set(contracts)
    failures: list[str] = []
    if actual != expected:
        failures.append(
            "workflow execution policy inventory differs from repository "
            f"(missing={sorted(expected - actual)}, unclassified={sorted(actual - expected)})"
        )
    for path in workflows:
        relative = path.resolve().relative_to(repo_root).as_posix()
        if relative not in contracts:
            continue
        failures.extend(
            f"{relative}: {failure}"
            for failure in workflow_failures(path, contracts[relative], maximum)
        )
    return failures


def main(argv: list[str] | None = None) -> int:
    entry = cli.Entrypoint("Workflow execution policy", __doc__)
    entry.parser.add_argument("repository_root", nargs="?", type=Path, default=cli.REPO_ROOT)
    entry.parser.add_argument("--workflow-root", type=Path)
    entry.parser.add_argument("--policy", type=Path)
    arguments = entry.parse(argv)

    try:
        repo_root = arguments.repository_root.resolve()
        workflow_root = (
            arguments.workflow_root or repo_root / ".github/workflows"
        ).resolve()
        policy_path = (
            arguments.policy or repo_root / ".github/policies/workflow-execution.json"
        ).resolve()
        failures = violations(repo_root, workflow_root, policy_path, arguments.today)
    except cli.FAILING as error:
        return entry.failed_closed(error)

    return entry.report(
        failures,
        "Workflow permissions, concurrency, and job timeouts match policy "
        f"(evaluated {arguments.today.isoformat()})",
    )


if __name__ == "__main__":
    raise SystemExit(main())
