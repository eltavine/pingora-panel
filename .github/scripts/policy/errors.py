"""The single failure type shared by every repository policy guard."""

from __future__ import annotations


class PolicyError(ValueError):
    """A malformed policy document, or a tool report that cannot be trusted.

    Guards translate this into exit code 2 ("failed closed") so that an
    unreadable policy is never mistaken for an absence of findings.
    """


__all__ = ["PolicyError"]
