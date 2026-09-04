"""How cargo-deny findings are governed, per workspace.

A workspace can only be held to an exact set of findings when its resolution is
reproducible, which in practice means a committed lockfile. The vendored
upstream workspace deliberately does not commit one, so it re-resolves on every
run: not just the versions but the very set of duplicated crates changes when an
unrelated dependency publishes. Demanding an exact lease set there produces
failures that carry no information.

Two enforcement modes therefore exist, and each workspace declares the one that
matches its reproducibility:

* ``identity`` — every finding holds its own lease naming the exact versions.
  Any change requires review. Requires a committed lockfile.
* ``ceilings`` — each finding kind carries a leased maximum count. Resolution
  churn moves the count a little and passes; a dependency change that makes the
  situation materially worse fails. A kind with no declared ceiling tolerates
  nothing.

Adding a mode is one :data:`MODES` entry.
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import date
from typing import Callable, Mapping, Protocol, Sequence

from . import advisory_ids, fields, leases
from .cargo_deny import Finding, FindingKind
from .errors import PolicyError

LeaseKey = tuple[str, ...]


@dataclass(frozen=True)
class LeasedKind:
    """One cargo-deny finding kind together with the shape of its leases."""

    kind: FindingKind
    identified_by_advisory: bool
    minimum_versions: int = 1

    @property
    def code(self) -> str:
        return self.kind.code

    @property
    def section(self) -> str:
        return self.kind.section

    def subject_fields(self) -> frozenset[str]:
        names = {"crate", "versions"}
        if self.identified_by_advisory:
            names.add("advisory_id")
        return frozenset(names)

    def read_subject(self, entry: Mapping[str, object], context: str) -> Finding:
        versions = fields.sorted_unique_versions(entry.get("versions"), f"{context} versions")
        if len(versions) < self.minimum_versions:
            raise PolicyError(
                f"{context} must lease at least {self.minimum_versions} versions"
            )
        return Finding(
            code=self.code,
            crate=fields.crate_name(entry.get("crate"), f"{context} crate"),
            versions=versions,
            advisory_id=(
                advisory_ids.canonical(entry.get("advisory_id"), f"{context} advisory_id")
                if self.identified_by_advisory
                else None
            ),
        )


def finding_key(finding: Finding) -> LeaseKey:
    return (
        finding.code,
        finding.advisory_id or "",
        finding.crate or "",
        *finding.versions,
    )


class Enforcement(Protocol):
    """Evaluates one workspace's observed findings against its declared policy."""

    def failures(self, observed: Sequence[Finding]) -> list[str]: ...

    def summary(self, observed: Sequence[Finding]) -> str: ...


@dataclass(frozen=True)
class IdentityLeases:
    """Requires one live lease per finding, and no lease without a finding."""

    holders: Mapping[LeaseKey, str]
    subjects: Mapping[LeaseKey, str]

    def failures(self, observed: Sequence[Finding]) -> list[str]:
        seen = {finding_key(finding): finding for finding in observed}
        reported = [
            f"unleased {finding.code} finding: {finding.describe()}"
            for key, finding in sorted(seen.items())
            if key not in self.holders
        ]
        reported.extend(
            f"stale lease held by {self.holders[key]}: {self.subjects[key]}"
            for key in sorted(self.holders)
            if key not in seen
        )
        return reported

    def summary(self, observed: Sequence[Finding]) -> str:
        leased = {finding_key(finding) for finding in observed}
        return f"{len(leased)} findings each hold an exact-version lease"


@dataclass(frozen=True)
class CountCeilings:
    """Requires each finding kind to stay within its leased maximum count.

    A ceiling is reviewed on its expiry date rather than by comparing it against
    the live count. Flagging a ceiling as "too generous" would have to measure
    the gap to the current count, and that count is exactly the quantity this
    mode exists to stop depending on: an unrelated upstream release moves it, so
    the check would fail for reasons carrying no information. Time-driven review
    is the only schedule an untracked lockfile can honour.
    """

    ceilings: Mapping[str, int]
    holders: Mapping[str, str]

    def _counts(self, observed: Sequence[Finding]) -> dict[str, int]:
        counts: dict[str, int] = {}
        for finding in observed:
            counts[finding.code] = counts.get(finding.code, 0) + 1
        return counts

    def failures(self, observed: Sequence[Finding]) -> list[str]:
        counts = self._counts(observed)
        reported: list[str] = []
        for code, count in sorted(counts.items()):
            ceiling = self.ceilings.get(code)
            if ceiling is None:
                reported.append(
                    f"{count} {code} findings with no declared ceiling; "
                    "an undeclared finding kind tolerates nothing"
                )
            elif count > ceiling:
                reported.append(
                    f"{count} {code} findings exceed the ceiling of {ceiling} "
                    f"leased by {self.holders[code]}"
                )
        return reported

    def summary(self, observed: Sequence[Finding]) -> str:
        counts = self._counts(observed)
        stated = ", ".join(
            f"{counts.get(code, 0)}/{ceiling} {code}"
            for code, ceiling in sorted(self.ceilings.items())
        )
        return f"findings stay within their leased ceilings ({stated})"


def _read_identity(
    workspace: str,
    entry: Mapping[str, object],
    kinds: Sequence[LeasedKind],
    today: date,
) -> Enforcement:
    holders: dict[LeaseKey, str] = {}
    subjects: dict[LeaseKey, str] = {}
    for leased in sorted(kinds, key=lambda leased: leased.section):
        entries = entry.get(leased.section, [])
        if not isinstance(entries, list):
            raise PolicyError(f"{workspace} {leased.section} must be a list of leases")
        for index, raw in enumerate(entries):
            context = f"{workspace} {leased.section}[{index}]"
            mapping, accountability = leases.read_lease(
                raw, leased.subject_fields(), context, today
            )
            subject = leased.read_subject(mapping, context)
            key = finding_key(subject)
            if key in holders:
                raise PolicyError(
                    f"{workspace} {leased.section} leases {subject.describe()} twice"
                )
            holders[key] = accountability.owner
            subjects[key] = f"{leased.section} {subject.describe()}"
    return IdentityLeases(holders, subjects)


def _read_ceilings(
    workspace: str,
    entry: Mapping[str, object],
    kinds: Sequence[LeasedKind],
    today: date,
) -> Enforcement:
    declared = entry.get("ceilings")
    if not isinstance(declared, dict) or not declared:
        raise PolicyError(f"{workspace} must declare at least one finding ceiling")
    sections = {leased.section: leased for leased in kinds}
    unknown = set(declared) - set(sections)
    if unknown:
        raise PolicyError(f"{workspace} declares unknown ceilings: {sorted(unknown)}")
    ceilings: dict[str, int] = {}
    holders: dict[str, str] = {}
    for section, raw in sorted(declared.items()):
        context = f"{workspace} ceilings.{section}"
        mapping, accountability = leases.read_lease(
            raw, frozenset({"max_findings"}), context, today
        )
        maximum = mapping["max_findings"]
        if not isinstance(maximum, int) or isinstance(maximum, bool) or maximum < 0:
            raise PolicyError(f"{context} max_findings must be a non-negative integer")
        code = sections[section].code
        ceilings[code] = maximum
        holders[code] = accountability.owner
    return CountCeilings(ceilings, holders)


EnforcementReader = Callable[
    [str, Mapping[str, object], Sequence[LeasedKind], date], Enforcement
]


@dataclass(frozen=True)
class EnforcementMode:
    """One named way to govern a workspace's findings."""

    name: str
    read: EnforcementReader
    #: The reproducibility this mode is defined for. A workspace must declare a
    #: mode matching the fact Git reports, so this is an equality rather than a
    #: minimum: a mode is never merely permitted, it is the right one or wrong.
    applies_to_reproducible: bool
    rationale: str
    #: Sections this mode owns. Empty means one section per finding kind.
    fixed_sections: frozenset[str] = frozenset()

    def sections(self, kinds: Sequence[LeasedKind]) -> frozenset[str]:
        """Return the document sections this mode may use."""
        if self.fixed_sections:
            return self.fixed_sections
        return frozenset(leased.section for leased in kinds)


IDENTITY = EnforcementMode(
    name="identity",
    read=_read_identity,
    applies_to_reproducible=True,
    rationale=(
        "an exact lease per finding is only meaningful when the lockfile is "
        "committed, because otherwise every re-resolution invalidates it"
    ),
)

CEILINGS = EnforcementMode(
    name="ceilings",
    read=_read_ceilings,
    applies_to_reproducible=False,
    rationale=(
        "a committed lockfile supports exact leases, so a count ceiling would "
        "accept silent substitutions it could have caught"
    ),
    fixed_sections=frozenset({"ceilings"}),
)

MODES = {mode.name: mode for mode in (IDENTITY, CEILINGS)}


def supported() -> tuple[str, ...]:
    return tuple(sorted(MODES))


def read(
    workspace: str,
    entry: Mapping[str, object],
    kinds: Sequence[LeasedKind],
    today: date,
    reproducible: bool,
) -> Enforcement:
    """Select and build the enforcement one workspace declared.

    The declared mode must match the workspace's actual reproducibility, which is
    read from Git rather than from this document. A registry therefore cannot
    claim a strictness the repository does not support, nor settle for a weaker
    one than it does.
    """
    mode_name = entry.get("enforcement")
    if not isinstance(mode_name, str) or mode_name not in MODES:
        raise PolicyError(
            f"{workspace} enforcement must be one of {supported()}: {mode_name!r}"
        )
    mode = MODES[mode_name]
    if mode.applies_to_reproducible != reproducible:
        raise PolicyError(
            f"{workspace} declares {mode_name} enforcement but has "
            f"{'a committed' if reproducible else 'an untracked'} lockfile; "
            f"{mode.rationale}"
        )
    allowed = {"enforcement", *mode.sections(kinds)}
    unknown = set(entry) - allowed
    if unknown:
        raise PolicyError(
            f"{workspace} declares sections unusable under {mode_name} "
            f"enforcement: {sorted(unknown)}"
        )
    return mode.read(workspace, entry, kinds, today)


__all__ = [
    "CEILINGS",
    "CountCeilings",
    "Enforcement",
    "EnforcementMode",
    "EnforcementReader",
    "IDENTITY",
    "IdentityLeases",
    "LeaseKey",
    "LeasedKind",
    "MODES",
    "finding_key",
    "read",
    "supported",
]
