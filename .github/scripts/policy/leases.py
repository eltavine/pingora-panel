"""The accountability contract every policy exception must satisfy.

An exception is only tolerable when someone owns it, wrote down why, and
committed to a review date. This module is the one place that decides what
those three fields mean, so every registry enforces them identically.

Both ends of the review date are enforced. Expiry is what makes a lease a lease
rather than a permanent grant with a comment attached, and an expiry far enough
out is a permanent grant: the mechanism reads as accountability while nobody
alive will be asked to justify it again. So a lease must expire, and must expire
soon enough that expiring means something.
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import date, timedelta

from . import fields, owners
from .errors import PolicyError

ACCOUNTABILITY_FIELDS = frozenset({"owner", "reason", "expires_on"})

#: How far ahead a lease may be set to expire. Long enough for an annual review
#: cycle with slack, short enough that every exception is re-argued by someone
#: who still remembers the system. Raising this is a decision about how long the
#: repository is willing to not revisit a thing, and belongs in review.
REVIEW_HORIZON = timedelta(days=400)


@dataclass(frozen=True)
class Accountability:
    """Who owns one exception, why it exists, and when it must be revisited."""

    owner: str
    reason: str
    expires_on: date

    @classmethod
    def read(
        cls,
        entry: dict[str, object],
        context: str,
        *,
        known_owners: owners.OwnerRegistry | None = None,
    ) -> "Accountability":
        registry = owners.REGISTERED if known_owners is None else known_owners
        return cls(
            owner=registry.require(entry.get("owner"), f"{context} owner"),
            reason=fields.text(entry.get("reason"), f"{context} reason"),
            expires_on=fields.iso_date(entry.get("expires_on"), f"{context} expires_on"),
        )

    def require_reviewable(self, context: str, today: date) -> "Accountability":
        """Require the lease to be in force now and to come up for review.

        The lease is valid through its expiry date itself, and is refused once
        that date has passed. It is also refused when the date is beyond
        :data:`REVIEW_HORIZON`, which is the case that looks like accountability
        and is not: a date nobody will be present for is the same as no date.
        """
        if self.expires_on < today:
            raise PolicyError(
                f"{context} lease expired on {self.expires_on.isoformat()} "
                f"(owner {self.owner})"
            )
        horizon = today + REVIEW_HORIZON
        if self.expires_on > horizon:
            raise PolicyError(
                f"{context} lease expires on {self.expires_on.isoformat()}, beyond "
                f"the {REVIEW_HORIZON.days}-day review horizon ending "
                f"{horizon.isoformat()}; an exception nobody will be asked to "
                f"justify again is a permanent grant (owner {self.owner})"
            )
        return self


def require_exact_fields(
    entry: object, expected: frozenset[str], context: str
) -> dict[str, object]:
    """Reject unknown and missing keys so a lease cannot carry silent metadata."""
    if not isinstance(entry, dict):
        raise PolicyError(f"{context} must be a mapping")
    actual = set(entry)
    if actual != set(expected):
        raise PolicyError(
            f"{context} has unexpected fields "
            f"(unknown={sorted(actual - expected)}, missing={sorted(expected - actual)})"
        )
    return entry


def read_lease(
    entry: object,
    subject_fields: frozenset[str],
    context: str,
    today: date,
    *,
    known_owners: owners.OwnerRegistry | None = None,
) -> tuple[dict[str, object], Accountability]:
    """Read one lease: its subject fields plus the accountability contract."""
    mapping = require_exact_fields(
        entry, frozenset(subject_fields | ACCOUNTABILITY_FIELDS), context
    )
    accountability = Accountability.read(
        mapping, context, known_owners=known_owners
    ).require_reviewable(context, today)
    return mapping, accountability


__all__ = [
    "ACCOUNTABILITY_FIELDS",
    "REVIEW_HORIZON",
    "Accountability",
    "read_lease",
    "require_exact_fields",
]
