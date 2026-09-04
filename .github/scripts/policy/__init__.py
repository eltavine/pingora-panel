"""Shared building blocks for the repository's policy guards.

Guards stay thin by composing independent concerns:

* :mod:`policy.fields` — primitive field validation shared by every document.
* :mod:`policy.advisory_ids` — which advisory naming schemes are accepted, and why.
* :mod:`policy.leases` — the owner/reason/expiry contract for any exception.
* :mod:`policy.owners` — who may be named accountable for one.
* :mod:`policy.registry` — schema-versioned document loading.
* :mod:`policy.workspaces` — whether a Cargo workspace resolves reproducibly.
* :mod:`policy.cargo_deny` — the cargo-deny report adapter and finding kinds.
* :mod:`policy.finding_leases` — how findings are governed, per workspace.
* :mod:`policy.advisories` — the Rust advisory exception registry.
* :mod:`policy.ci_yaml` — quote-aware scanning of workflow YAML.
* :mod:`policy.installers` — immutability rules for CI package installers.
* :mod:`policy.cli` — the exit-code contract and shared options every guard presents.

The lower four modules know nothing about a specific policy file. Each guard
declares its own document shape and reuses these pieces, so a tightened rule
lands everywhere at once instead of drifting between scripts.
"""

from __future__ import annotations

from .errors import PolicyError

__all__ = ["PolicyError"]
