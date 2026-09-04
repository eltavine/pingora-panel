"""Who may be named accountable for a policy exception.

Every lease in this repository names an owner, and the owner is the whole point:
it is the answer to "who decided this was acceptable, and who do I ask when it
expires". A free-text field cannot answer that. `pingora-panel-securty` reads
correctly to a reviewer skimming a diff, is accepted without complaint, and
points at nobody — so the exception is unowned while looking owned.

Owners are registered rather than validated by pattern, because there is no
pattern: the set is small, known, and changes deliberately. Adding a team is one
entry here, which is also the reviewable moment to say what it is accountable
for.

This module deliberately knows nothing about leases. `policy.leases` consults it
by default and accepts an override, so a self-test can exercise the accountability
contract against its own owners without the real org chart leaking into fixtures.
"""

from __future__ import annotations

from dataclasses import dataclass

from . import fields
from .errors import PolicyError


@dataclass(frozen=True)
class Owner:
    """One team that may be named accountable, and what it answers for."""

    name: str
    accountable_for: str


class OwnerRegistry:
    """The owners a lease may name."""

    def __init__(self, *owners: Owner) -> None:
        self._owners: dict[str, Owner] = {}
        for owner in owners:
            self.register(owner)

    def register(self, owner: Owner) -> None:
        if owner.name in self._owners:
            raise ValueError(f"owner {owner.name} is already registered")
        self._owners[owner.name] = owner

    def names(self) -> tuple[str, ...]:
        return tuple(sorted(self._owners))

    def accountable_for(self, name: str) -> str:
        return self._owners[name].accountable_for

    def require(self, value: object, field: str = "owner") -> str:
        """Return the owner's name, or refuse and say who could have been named."""
        candidate = fields.text(value, field)
        if candidate not in self._owners:
            raise PolicyError(
                f"{field} names nobody accountable: {candidate!r} "
                f"(registered: {', '.join(self.names())})"
            )
        return candidate


PLATFORM = Owner(
    name="pingora-panel-platform",
    accountable_for="the build, CI, and dependency hygiene",
)

SECURITY = Owner(
    name="pingora-panel-security",
    accountable_for="advisories, secrets, and privilege grants",
)

#: The owners in force. Naming a new team means registering it here.
REGISTERED = OwnerRegistry(PLATFORM, SECURITY)


__all__ = ["PLATFORM", "REGISTERED", "SECURITY", "Owner", "OwnerRegistry"]
