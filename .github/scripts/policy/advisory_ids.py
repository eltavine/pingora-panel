"""Which advisory naming schemes this repository will accept.

`RUSTSEC-2026-0098` is not the only way to name a vulnerability affecting a Rust
crate. GHSA and CVE identifiers name the same advisories, and both `cargo audit`
and `cargo deny` will accept one in an ignore list without complaint.

Accepting only RUSTSEC is a decision rather than an omission. The guards here
prove that every ignored advisory is one cargo-deny actually resolved against
the crate graph, and cargo-deny resolves against the RustSec database: an entry
named by any other scheme suppresses nothing, so it would be counted as an
applied exception while protecting nobody.

That decision used to be a bare regex in the field validators, which said
nothing about why, and told a maintainer who wrote a GHSA identifier that their
entry was "malformed" — as if they had mistyped it. Registering schemes instead
makes admitting one a reviewable line, and makes refusing one carry its reason
to the person who tried.
"""

from __future__ import annotations

import re
from dataclasses import dataclass

from . import fields
from .errors import PolicyError


@dataclass(frozen=True)
class Scheme:
    """One advisory naming scheme, and this repository's stance on it."""

    name: str
    pattern: re.Pattern[str]
    accepted: bool
    rationale: str


class SchemeRegistry:
    """The schemes a policy document may name an advisory with."""

    def __init__(self, *schemes: Scheme) -> None:
        self._schemes: dict[str, Scheme] = {}
        for scheme in schemes:
            self.register(scheme)

    def register(self, scheme: Scheme) -> None:
        if scheme.name in self._schemes:
            raise ValueError(f"advisory scheme {scheme.name} is already registered")
        self._schemes[scheme.name] = scheme

    def accepted(self) -> tuple[str, ...]:
        return tuple(
            sorted(name for name, scheme in self._schemes.items() if scheme.accepted)
        )

    def canonical(self, value: object, field: str = "advisory_id") -> str:
        """Return the identifier, or explain why this repository will not take it.

        A recognised-but-refused scheme is reported with the reason it is out of
        scope, so the answer is actionable. An identifier matching nothing is
        reported as naming no known database, which is the honest reading: it
        may be a typo, or a scheme nobody here has considered.
        """
        candidate = fields.text(value, field)
        for scheme in self._schemes.values():
            if scheme.pattern.fullmatch(candidate) is None:
                continue
            if scheme.accepted:
                return candidate
            raise PolicyError(
                f"{field} names a {scheme.name} advisory, which this repository "
                f"does not accept: {scheme.rationale} ({candidate})"
            )
        raise PolicyError(
            f"{field} matches no known advisory scheme: {candidate!r} "
            f"(accepted: {', '.join(self.accepted())})"
        )


RUSTSEC = Scheme(
    name="RUSTSEC",
    pattern=re.compile(r"RUSTSEC-\d{4}-\d{4}"),
    accepted=True,
    rationale="the database cargo-deny resolves advisories against",
)

GHSA = Scheme(
    name="GHSA",
    pattern=re.compile(r"GHSA-[2-9a-hjkmnp-z]{4}-[2-9a-hjkmnp-z]{4}-[2-9a-hjkmnp-z]{4}"),
    accepted=False,
    rationale=(
        "cargo-deny matches ignore entries against the RustSec database, so a "
        "GHSA entry suppresses nothing while counting as an applied exception; "
        "use the RUSTSEC identifier cross-referenced from the GHSA advisory"
    ),
)

CVE = Scheme(
    name="CVE",
    pattern=re.compile(r"CVE-\d{4}-\d{4,}"),
    accepted=False,
    rationale=(
        "cargo-deny matches ignore entries against the RustSec database, so a "
        "CVE entry suppresses nothing while counting as an applied exception; "
        "use the RUSTSEC identifier that references the CVE"
    ),
)

#: The schemes in force. Admitting a database means registering it here, with
#: the reason, in one place that every guard reads.
SCHEMES = SchemeRegistry(RUSTSEC, GHSA, CVE)


def canonical(value: object, field: str = "advisory_id") -> str:
    return SCHEMES.canonical(value, field)


__all__ = ["CVE", "GHSA", "RUSTSEC", "SCHEMES", "Scheme", "SchemeRegistry", "canonical"]
