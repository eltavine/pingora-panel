"""Adapter over the cargo-deny JSON report.

This is the only module that knows cargo-deny's wire format. Guards consume
:class:`Finding` values, so a cargo-deny output change is absorbed here instead
of rippling through every policy script.

Every finding kind is registered rather than hard-coded, so leasing a new
diagnostic code is a one-entry change.
"""

from __future__ import annotations

import json
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Iterable, Mapping, Sequence

from . import advisory_ids, fields
from .errors import PolicyError

DIAGNOSTIC = "diagnostic"
SUMMARY = "summary"


@dataclass(frozen=True)
class Finding:
    """One leasable cargo-deny observation, normalized across finding kinds."""

    code: str
    crate: str | None = None
    versions: tuple[str, ...] = ()
    advisory_id: str | None = None

    def describe(self) -> str:
        subject = " ".join(part for part in (self.advisory_id, self.crate) if part)
        versions = f" [{', '.join(self.versions)}]" if self.versions else ""
        return f"{subject}{versions}"


@dataclass(frozen=True)
class FindingKind:
    """Binds one cargo-deny diagnostic code to a policy document section."""

    code: str
    section: str
    extract: Callable[[Mapping[str, object]], Finding]


class FindingKindRegistry:
    """The set of cargo-deny diagnostic codes a guard is willing to interpret."""

    def __init__(self, *kinds: FindingKind) -> None:
        self._kinds: dict[str, FindingKind] = {}
        for kind in kinds:
            self.register(kind)

    def register(self, kind: FindingKind) -> None:
        if kind.code in self._kinds:
            raise ValueError(f"cargo-deny code {kind.code} is already registered")
        self._kinds[kind.code] = kind

    def codes(self) -> tuple[str, ...]:
        return tuple(sorted(self._kinds))

    def sections(self) -> tuple[str, ...]:
        return tuple(sorted({kind.section for kind in self._kinds.values()}))

    def section_of(self, code: str) -> str:
        return self._kinds[code].section

    def get(self, code: str) -> FindingKind | None:
        return self._kinds.get(code)


def _graph_crates(report: Mapping[str, object]) -> tuple[str, tuple[str, ...]]:
    graphs = report.get("graphs")
    if not isinstance(graphs, list) or not graphs:
        raise PolicyError("cargo-deny reported a finding without crate graphs")
    names: set[str] = set()
    versions: set[str] = set()
    for graph in graphs:
        if not isinstance(graph, dict) or not isinstance(graph.get("Krate"), dict):
            raise PolicyError("cargo-deny reported a finding without a crate graph")
        crate = graph["Krate"]
        names.add(fields.crate_name(crate.get("name"), "cargo-deny crate name"))
        versions.add(
            fields.matching(
                crate.get("version"), "cargo-deny crate version", fields.CRATE_VERSION
            )
        )
    if len(names) != 1:
        raise PolicyError(
            f"cargo-deny grouped one finding across crates: {sorted(names)}"
        )
    return names.pop(), tuple(sorted(versions, key=fields.version_order))


def _advisory_identifier(report: Mapping[str, object]) -> str:
    advisory = report.get("advisory")
    if isinstance(advisory, dict):
        return advisory_ids.canonical(advisory.get("id"), "cargo-deny advisory id")
    raise PolicyError("cargo-deny reported an advisory finding without an advisory")


def _labelled_identifier(report: Mapping[str, object]) -> str:
    labels = report.get("labels")
    if not isinstance(labels, list) or len(labels) != 1 or not isinstance(labels[0], dict):
        raise PolicyError("cargo-deny reported an unmatched advisory without one label")
    return advisory_ids.canonical(labels[0].get("span"), "cargo-deny advisory label")


def crate_finding(code: str) -> Callable[[Mapping[str, object]], Finding]:
    """Extractor for findings identified by a crate and its resolved versions."""

    def extract(report: Mapping[str, object]) -> Finding:
        crate, versions = _graph_crates(report)
        return Finding(code=code, crate=crate, versions=versions)

    return extract


def advisory_crate_finding(code: str) -> Callable[[Mapping[str, object]], Finding]:
    """Extractor for advisory findings that also name an affected crate."""

    def extract(report: Mapping[str, object]) -> Finding:
        crate, versions = _graph_crates(report)
        return Finding(
            code=code,
            crate=crate,
            versions=versions,
            advisory_id=_advisory_identifier(report),
        )

    return extract


def unmatched_advisory_finding(code: str) -> Callable[[Mapping[str, object]], Finding]:
    """Extractor for ignore entries that matched no crate in the graph."""

    def extract(report: Mapping[str, object]) -> Finding:
        return Finding(code=code, advisory_id=_labelled_identifier(report))

    return extract


DUPLICATE = FindingKind("duplicate", "duplicates", crate_finding("duplicate"))
UNMAINTAINED = FindingKind(
    "unmaintained", "unmaintained", advisory_crate_finding("unmaintained")
)
YANKED = FindingKind("yanked", "yanked", crate_finding("yanked"))
ADVISORY_NOT_DETECTED = FindingKind(
    "advisory-not-detected",
    "unmatched",
    unmatched_advisory_finding("advisory-not-detected"),
)

#: Codes that make the report's silence meaningless. cargo-deny emits each of
#: these as a non-blocking warning and still exits successfully, so a guard that
#: skipped them would draw a conclusion from a check that never ran.
INVALIDATING: Mapping[str, str] = {
    "index-failure": (
        "cargo-deny could not query a registry index, so it reported no yanked "
        "crates without having looked for one"
    ),
    "index-cache-load-failure": (
        "cargo-deny could not load its cached registry index, so yanked "
        "detection did not run"
    ),
    "unknown-advisory": (
        "an ignore entry names an advisory that exists in no database, so it "
        "suppresses nothing yet would be counted as an applied exception"
    ),
    "yanked-not-detected": (
        "an ignore entry for a yanked crate matched nothing, so the exception "
        "is stale"
    ),
}

#: Every finding kind any guard in this repository interprets. A guard reads a
#: subset, but the report they read is produced once for the whole set, so the
#: union is declared here rather than inferred from whoever happens to run.
KNOWN: tuple[FindingKind, ...] = (
    DUPLICATE,
    UNMAINTAINED,
    YANKED,
    ADVISORY_NOT_DETECTED,
)

#: The cargo-deny checks that between them produce every kind in :data:`KNOWN`.
#: One run covering these serves every guard, so two guards examining the same
#: commit cannot resolve the graph differently and disagree about it.
ALL_CHECKS: tuple[str, ...] = ("advisories", "bans")

#: Codes one guard interprets while another deliberately passes over them. A
#: code outside this set and :data:`INVALIDATING` is unclassified, and an
#: unclassified finding is refused rather than skipped: a new cargo-deny
#: diagnostic must be given a meaning here before any guard reports success on a
#: report containing it.
TOLERATED: frozenset[str] = frozenset(kind.code for kind in KNOWN)

#: Severities that carry policy weight. Notes and help text are commentary, so
#: an unrecognised one is passed over instead of failing the run.
ACTIONABLE = frozenset({"warning", "error"})
DIAGNOSTIC_SEVERITIES = frozenset({"error", "warning", "note", "help"})


def parse_report(
    lines: Iterable[str], kinds: FindingKindRegistry, checks: Sequence[str]
) -> tuple[Finding, ...]:
    """Extract registered findings, requiring cargo-deny's completion summary.

    The summary is the fail-closed anchor: without it, a cargo-deny that died
    before evaluating the graph would look indistinguishable from a clean run.

    An unclassified diagnostic is likewise refused. Skipping one would turn a
    finding nobody has reasoned about into silence, and silence is what every
    guard here reads as success.
    """
    findings: list[Finding] = []
    summaries = 0
    for line in lines:
        stripped = line.strip()
        if not stripped.startswith("{"):
            continue
        try:
            message = json.loads(stripped)
        except json.JSONDecodeError as error:
            raise PolicyError("cargo-deny emitted a malformed JSON report line") from error
        if not isinstance(message, dict):
            continue
        message_type = message.get("type")
        if message_type not in {SUMMARY, DIAGNOSTIC}:
            continue
        report = message.get("fields")
        if not isinstance(report, dict):
            raise PolicyError(
                f"cargo-deny {message_type} report has no fields mapping"
            )
        if message_type == SUMMARY:
            missing = [check for check in checks if check not in report]
            if missing:
                raise PolicyError(f"cargo-deny summary omitted checks: {missing}")
            malformed = [
                check
                for check in checks
                if not isinstance(report[check], dict)
                or any(
                    not isinstance(count, int) or isinstance(count, bool) or count < 0
                    for count in report[check].values()
                )
            ]
            if malformed:
                raise PolicyError(f"cargo-deny summary malformed checks: {malformed}")
            summaries += 1
            continue
        code = report.get("code")
        severity = report.get("severity")
        if not isinstance(code, str) or not code:
            raise PolicyError("cargo-deny emitted a diagnostic without a code")
        if severity not in DIAGNOSTIC_SEVERITIES:
            raise PolicyError(
                f"cargo-deny diagnostic {code!r} has an unknown severity: {severity!r}"
            )
        if code in INVALIDATING:
            raise PolicyError(f"cargo-deny reported {code}: {INVALIDATING[code]}")
        kind = kinds.get(code)
        if kind is not None:
            findings.append(kind.extract(report))
        elif code not in TOLERATED and severity in ACTIONABLE:
            raise PolicyError(
                f"cargo-deny reported the unclassified diagnostic {code!r}; "
                "give it a meaning in policy/cargo_deny.py before trusting a "
                "report that contains it"
            )
    if summaries != 1:
        raise PolicyError(
            f"cargo-deny did not run to completion ({summaries} summary reports)"
        )
    return tuple(findings)


def run(manifest: Path, repo_root: Path, checks: Sequence[str]) -> list[str]:
    """Invoke cargo-deny for one workspace and return its report lines.

    Status 1 is deliberately accepted: cargo-deny fails the build for its own
    blocking findings in a separate step, and this adapter must still read the
    leasable warnings from that report. Other statuses indicate invocation or
    runtime failure and cannot support a trustworthy policy conclusion.
    """
    command = [
        "cargo",
        "deny",
        "--manifest-path",
        str(manifest),
        "--format",
        "json",
        "check",
        *checks,
    ]
    try:
        completed = subprocess.run(
            command, cwd=repo_root, check=False, capture_output=True, text=True
        )
    except OSError as error:
        raise PolicyError(f"cannot execute cargo-deny: {error}") from error
    if completed.returncode not in {0, 1}:
        raise PolicyError(
            f"cargo-deny exited with unexpected status {completed.returncode}"
        )
    return (completed.stdout + completed.stderr).splitlines()


__all__ = [
    "ACTIONABLE",
    "DIAGNOSTIC_SEVERITIES",
    "ALL_CHECKS",
    "ADVISORY_NOT_DETECTED",
    "DUPLICATE",
    "Finding",
    "FindingKind",
    "FindingKindRegistry",
    "INVALIDATING",
    "KNOWN",
    "TOLERATED",
    "UNMAINTAINED",
    "YANKED",
    "advisory_crate_finding",
    "crate_finding",
    "parse_report",
    "run",
    "unmatched_advisory_finding",
]
