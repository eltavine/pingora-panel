"""The Rust advisory exception registry, shared by the guards that read it.

Two independent guards consume this document: one proves the registry stays
synchronized with the cargo-audit and cargo-deny ignore lists, and one proves
each exception is actually applied in exactly the workspaces that claim it.
Both read the document through this module so the schema is defined once.
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import date
from pathlib import Path

from . import advisory_ids, fields, leases, registry
from .errors import PolicyError

SUBJECT_FIELDS = frozenset({"advisory_id", "scope", "workspaces"})


@dataclass(frozen=True)
class AdvisoryException:
    """One leased advisory ignore, and the workspaces it is claimed to cover."""

    advisory_id: str
    workspaces: tuple[str, ...]
    scope: str
    owner: str

    def claims(self, workspace: str) -> bool:
        return workspace in self.workspaces


DOCUMENTS: registry.DocumentRegistry[date, tuple[AdvisoryException, ...]] = (
    registry.DocumentRegistry("advisory exception registry")
)


def _read_exception(entry: object, index: int, today: date) -> AdvisoryException:
    context = f"advisory exception[{index}]"
    mapping, accountability = leases.read_lease(entry, SUBJECT_FIELDS, context, today)
    identifier = advisory_ids.canonical(mapping.get("advisory_id"), f"{context} advisory_id")
    claimed = mapping.get("workspaces")
    if not isinstance(claimed, list) or not claimed:
        raise PolicyError(
            f"{context} must claim at least one workspace; an exception that "
            "applies nowhere must be deleted instead"
        )
    workspaces = tuple(
        fields.manifest_path(value, f"{context} workspaces entry") for value in claimed
    )
    if len(set(workspaces)) != len(workspaces) or list(workspaces) != sorted(workspaces):
        raise PolicyError(f"{context} workspaces must be unique and sorted")
    return AdvisoryException(
        advisory_id=identifier,
        workspaces=workspaces,
        scope=fields.text(mapping.get("scope"), f"{context} scope"),
        owner=accountability.owner,
    )


@DOCUMENTS.reader(2)
def _read_v2(document: dict, today: date) -> tuple[AdvisoryException, ...]:
    """Read the v2 contract, which requires an explicit workspace claim.

    v1 carried no workspace claim and is intentionally unsupported: a document
    without claims cannot prove that an exception is still reachable, which is
    the property the scope guard exists to enforce.
    """
    if set(document) != {"schema_version", "exceptions"}:
        raise PolicyError(
            "advisory exception registry v2 must contain exactly schema_version and exceptions"
        )
    entries = document["exceptions"]
    if not isinstance(entries, list):
        raise PolicyError("advisory exception registry exceptions must be a list")
    exceptions = tuple(
        _read_exception(entry, index, today) for index, entry in enumerate(entries)
    )
    identifiers = [exception.advisory_id for exception in exceptions]
    if len(set(identifiers)) != len(identifiers):
        raise PolicyError("advisory exception registry contains duplicate advisory IDs")
    return exceptions


def load(path: Path, today: date) -> tuple[AdvisoryException, ...]:
    """Load and fully validate the registry against one evaluation date."""
    return DOCUMENTS.load(path, today)


__all__ = ["AdvisoryException", "DOCUMENTS", "SUBJECT_FIELDS", "load"]
