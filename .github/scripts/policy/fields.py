"""Primitive field validators shared by every policy document reader.

Each validator either returns the canonical value or raises
:class:`~policy.errors.PolicyError`. Keeping them here means a tightened rule
applies to every policy file at once instead of drifting per guard.
"""

from __future__ import annotations

import re
import unicodedata
from datetime import date
from pathlib import PurePosixPath

from .errors import PolicyError

CRATE_NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]*")
CRATE_VERSION = re.compile(r"\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?")
VERSION_PARTS = re.compile(r"(\d+)\.(\d+)\.(\d+)(?:([-+])([0-9A-Za-z.-]+))?")
IDENTIFIER = re.compile(r"[A-Za-z0-9][A-Za-z0-9_-]*")


def displays_as_written(candidate: str) -> str | None:
    """Return why text would not render as its source reads, or None.

    A policy document is read by a human deciding whether to accept an
    exception, so the bytes and the rendering have to agree. A bidirectional
    override can make a reason display in an order its source does not have, and
    a zero-width or invisible formatting character can hide words entirely. Both
    are cheap to write and impossible to see in review, so they are refused
    rather than trusted to a careful reader.

    Control characters below the space are refused for the same reason: a
    carriage return or a backspace rewrites the line a reviewer sees.
    """
    for index, character in enumerate(candidate):
        code = ord(character)
        if code < 0x20 or code == 0x7F:
            return f"control character U+{code:04X} at offset {index}"
        if unicodedata.category(character) == "Cf":
            return f"invisible formatting character U+{code:04X} at offset {index}"
    return None


def text(value: object, field: str) -> str:
    """Require non-empty, single-line, untrimmed-whitespace-free text."""
    if not isinstance(value, str) or not value or value != value.strip():
        raise PolicyError(f"{field} must be non-empty single-line text")
    hidden = displays_as_written(value)
    if hidden is not None:
        raise PolicyError(f"{field} must display as written: {hidden}")
    return value


def matching(value: object, field: str, pattern: re.Pattern[str]) -> str:
    candidate = text(value, field)
    if pattern.fullmatch(candidate) is None:
        raise PolicyError(f"{field} is malformed: {candidate!r}")
    return candidate


def crate_name(value: object, field: str = "crate") -> str:
    return matching(value, field, CRATE_NAME)


def identifier(value: object, field: str) -> str:
    return matching(value, field, IDENTIFIER)


def iso_date(value: object, field: str) -> date:
    """Require a canonical ``YYYY-MM-DD`` date, rejecting lenient spellings."""
    candidate = text(value, field)
    try:
        parsed = date.fromisoformat(candidate)
    except ValueError as error:
        raise PolicyError(f"{field} is not an ISO date: {candidate!r}") from error
    if parsed.isoformat() != candidate:
        raise PolicyError(f"{field} is not a canonical ISO date: {candidate!r}")
    return parsed


def _prerelease_order(suffix: str) -> tuple[tuple[int, int, str], ...]:
    """Order semver pre-release identifiers: numeric ones numerically, and below
    alphanumeric ones."""
    return tuple(
        (0, int(part), "") if part.isdigit() else (1, 0, part)
        for part in suffix.split(".")
    )


def version_order(candidate: str) -> tuple[int, int, int, int, tuple, str]:
    """Sort key placing exact crate versions in release order, not string order.

    String order puts `1.0.109` before `1.0.9`, so a lease listing versions the
    way a maintainer would write them was refused, and the order it demanded
    instead was one nobody would choose. Numeric components are compared as
    numbers, and a pre-release sorts below the release it precedes.

    Build metadata carries no precedence under semver, but it is kept in the key
    so the order stays total: two versions differing only in build metadata must
    still sort deterministically, or the same lease could be spelled two ways.
    """
    match = VERSION_PARTS.fullmatch(candidate)
    if match is None:
        raise PolicyError(f"not an exact crate version: {candidate!r}")
    separator, suffix = match.group(4), match.group(5) or ""
    prerelease = separator == "-"
    return (
        int(match.group(1)),
        int(match.group(2)),
        int(match.group(3)),
        0 if prerelease else 1,
        _prerelease_order(suffix) if prerelease else (),
        "" if prerelease else suffix,
    )


def sorted_unique_versions(value: object, field: str) -> tuple[str, ...]:
    """Require a canonical, non-empty set of exact versions in release order."""
    if not isinstance(value, list) or not value:
        raise PolicyError(f"{field} must be a non-empty list of exact versions")
    versions = tuple(
        matching(entry, f"{field} entry", CRATE_VERSION) for entry in value
    )
    expected = sorted(versions, key=version_order)
    if len(set(versions)) != len(versions) or list(versions) != expected:
        raise PolicyError(
            f"{field} must be unique and in release order: expected {expected}"
        )
    return versions


def relative_posix_path(value: object, field: str) -> PurePosixPath:
    """Require a canonical, repository-relative POSIX path."""
    candidate = text(value, field)
    path = PurePosixPath(candidate)
    if (
        path.is_absolute()
        or path.as_posix() != candidate
        or "\\" in candidate
        or any(part in {"", ".", ".."} for part in path.parts)
    ):
        raise PolicyError(f"{field} must be a canonical relative path: {candidate!r}")
    return path


def manifest_path(value: object, field: str = "workspace") -> str:
    """Require a repository-relative path that names a Cargo manifest."""
    path = relative_posix_path(value, field)
    if path.name != "Cargo.toml":
        raise PolicyError(f"{field} must name a Cargo.toml manifest: {value!r}")
    return path.as_posix()


__all__ = [
    "CRATE_NAME",
    "CRATE_VERSION",
    "VERSION_PARTS",
    "IDENTIFIER",
    "crate_name",
    "displays_as_written",
    "identifier",
    "iso_date",
    "manifest_path",
    "matching",
    "relative_posix_path",
    "sorted_unique_versions",
    "version_order",
    "text",
]
