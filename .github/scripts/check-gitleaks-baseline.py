#!/usr/bin/env python3
"""Hold every Gitleaks allowlist entry to an owner, a reason, and an expiry.

`.gitleaksignore` is the file Gitleaks reads, but the lease registry is the
source of truth. The two must describe exactly the same fingerprints, so a
suppression cannot be added to the scanner without an accountable lease, and a
lease cannot linger after its fingerprint is dropped.

Each fingerprint is additionally proved to still point at a real line of a real
blob in a real commit, which is what stops a broad or dangling suppression.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from dataclasses import dataclass
from datetime import date, datetime, timezone
from pathlib import Path, PurePosixPath

from policy import PolicyError, fields, leases, registry

FINGERPRINT = re.compile(
    r"(?P<commit>[0-9a-f]{40}):(?P<path>[^:]+):"
    r"(?P<rule>[A-Za-z0-9][A-Za-z0-9_-]*):(?P<line>[1-9][0-9]*)"
)
SUBJECT_FIELDS = frozenset({"fingerprints"})


@dataclass(frozen=True)
class BaselinePolicy:
    """The leased fingerprint set and the ceiling on how far it may grow."""

    max_fingerprints: int
    holders: dict[str, str]


DOCUMENTS: registry.DocumentRegistry[date, BaselinePolicy] = registry.DocumentRegistry(
    "Gitleaks baseline registry"
)


@DOCUMENTS.reader(1)
def _read_v1(document: dict, today: date) -> BaselinePolicy:
    if set(document) != {"schema_version", "max_fingerprints", "leases"}:
        raise PolicyError(
            "Gitleaks baseline registry v1 must contain exactly schema_version, "
            "max_fingerprints, and leases"
        )
    ceiling = document["max_fingerprints"]
    if not isinstance(ceiling, int) or isinstance(ceiling, bool) or not 1 <= ceiling <= 256:
        raise PolicyError("Gitleaks baseline max_fingerprints must be between 1 and 256")
    entries = document["leases"]
    if not isinstance(entries, list) or not entries:
        raise PolicyError("Gitleaks baseline registry must hold at least one lease")

    holders: dict[str, str] = {}
    for index, entry in enumerate(entries):
        context = f"Gitleaks baseline lease[{index}]"
        mapping, accountability = leases.read_lease(entry, SUBJECT_FIELDS, context, today)
        fingerprints = mapping["fingerprints"]
        if not isinstance(fingerprints, list) or not fingerprints:
            raise PolicyError(f"{context} must list at least one fingerprint")
        for value in fingerprints:
            fingerprint = fields.matching(value, f"{context} fingerprint", FINGERPRINT)
            if fingerprint in holders:
                raise PolicyError(f"{context} repeats fingerprint {fingerprint}")
            holders[fingerprint] = accountability.owner
    if len(holders) > ceiling:
        raise PolicyError(
            f"Gitleaks baseline holds {len(holders)} fingerprints, "
            f"exceeding its own {ceiling} ceiling"
        )
    return BaselinePolicy(max_fingerprints=ceiling, holders=holders)


def git_output(repository: Path, *arguments: str) -> bytes | None:
    try:
        completed = subprocess.run(
            ["git", "-C", str(repository), *arguments], check=False, capture_output=True
        )
    except OSError as error:
        raise PolicyError(f"cannot invoke Git: {error}") from error
    return completed.stdout if completed.returncode == 0 else None


def baseline_fingerprints(baseline: Path) -> tuple[dict[str, int], list[str]]:
    """Read `.gitleaksignore`, returning each fingerprint's line and any failures."""
    try:
        lines = baseline.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeError) as error:
        raise PolicyError(f"cannot read {baseline}: {error}") from error
    found: dict[str, int] = {}
    failures: list[str] = []
    for line_number, raw_line in enumerate(lines, start=1):
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        if FINGERPRINT.fullmatch(line) is None:
            failures.append(
                f"{baseline}:{line_number}: entry must be one exact full fingerprint"
            )
            continue
        if line in found:
            failures.append(f"{baseline}:{line_number}: duplicate fingerprint")
            continue
        found[line] = line_number
    return found, failures


def unresolvable(repository: Path, fingerprint: str, location: str) -> str | None:
    """Report why one fingerprint does not name a real line of a real blob."""
    match = FINGERPRINT.fullmatch(fingerprint)
    if match is None:
        # Never reached through `validate`, but an `assert` would vanish under
        # `python -O` and let an unparsed fingerprint through unchecked.
        raise PolicyError(f"{location}: fingerprint is malformed: {fingerprint}")
    raw_path = match.group("path")
    path = PurePosixPath(raw_path)
    if (
        path.is_absolute()
        or ".." in path.parts
        or "\\" in raw_path
        or path.as_posix() != raw_path
    ):
        return f"{location}: path is not repository-relative"
    commit = match.group("commit")
    if git_output(repository, "cat-file", "-t", f"{commit}^{{commit}}") != b"commit\n":
        return f"{location}: commit does not exist: {commit}"
    object_name = f"{commit}:{path.as_posix()}"
    if git_output(repository, "cat-file", "-t", object_name) != b"blob\n":
        return f"{location}: path is not a file in commit: {path}"
    contents = git_output(repository, "cat-file", "-p", object_name)
    if contents is None:
        return f"{location}: cannot inspect file: {path}"
    available = (
        0 if not contents else contents.count(b"\n") + (0 if contents.endswith(b"\n") else 1)
    )
    if int(match.group("line")) > available:
        return (
            f"{location}: fingerprint line exceeds file length: "
            f"{path}:{match.group('line')}"
        )
    return None


def validate(repository: Path, baseline: Path, policy: BaselinePolicy) -> list[str]:
    found, failures = baseline_fingerprints(baseline)
    unleased = sorted(set(found) - set(policy.holders))
    stale = sorted(set(policy.holders) - set(found))
    failures.extend(
        f"{baseline}:{found[fingerprint]}: fingerprint has no lease in the registry"
        for fingerprint in unleased
    )
    failures.extend(
        f"registry lease held by {policy.holders[fingerprint]} is stale; "
        f"no baseline entry uses {fingerprint}"
        for fingerprint in stale
    )
    for fingerprint, line_number in sorted(found.items(), key=lambda item: item[1]):
        failure = unresolvable(repository, fingerprint, f"{baseline}:{line_number}")
        if failure is not None:
            failures.append(failure)
    if not found:
        failures.append(f"{baseline}: baseline must contain at least one fingerprint")
    return failures


def main(argv: list[str] | None = None) -> int:
    repo_root = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("repository", nargs="?", type=Path, default=repo_root)
    parser.add_argument("--baseline", type=Path, default=repo_root / ".gitleaksignore")
    parser.add_argument(
        "--registry",
        type=Path,
        default=repo_root / ".github/policies/gitleaks-baseline.json",
    )
    parser.add_argument(
        "--today",
        default=datetime.now(timezone.utc).date().isoformat(),
        help="UTC policy evaluation date in YYYY-MM-DD form",
    )
    arguments = parser.parse_args(argv)
    repository = arguments.repository.resolve()

    try:
        today = fields.iso_date(arguments.today, "--today")
        if git_output(repository, "rev-parse", "--is-inside-work-tree") != b"true\n":
            raise PolicyError(f"not a Git repository: {repository}")
        policy = DOCUMENTS.load(arguments.registry, today)
        failures = validate(repository, arguments.baseline.resolve(), policy)
    except (PolicyError, OSError) as error:
        print(f"Gitleaks baseline policy failed closed: {error}", file=sys.stderr)
        return 2
    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print(
        f"Gitleaks baseline verified ({len(policy.holders)} leased fingerprints "
        f"of {policy.max_fingerprints} permitted, evaluated {today.isoformat()})."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
