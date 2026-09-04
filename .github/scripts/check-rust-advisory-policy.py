#!/usr/bin/env python3
"""Keep cargo-audit and cargo-deny advisory exceptions exactly synchronized.

This guard proves the two ignore lists and the leased registry describe the same
set of advisories. Proving that each exception still applies where it claims to
is the separate job of `check-rust-advisory-scope.py`.
"""

from __future__ import annotations

import argparse
import sys
import tomllib
from datetime import datetime, timezone
from pathlib import Path

from policy import PolicyError, advisories, advisory_ids, fields


def ignored_advisories(path: Path) -> tuple[str, ...]:
    """Read one `[advisories] ignore` array from a cargo-audit or deny config."""
    try:
        with path.open("rb") as source:
            document = tomllib.load(source)
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise PolicyError(f"cannot read advisory configuration {path}: {error}") from error
    table = document.get("advisories")
    if not isinstance(table, dict):
        raise PolicyError(f"{path} has no [advisories] table")
    ignored = table.get("ignore")
    if not isinstance(ignored, list):
        raise PolicyError(f"{path} advisories.ignore must be an array")
    entries = tuple(
        advisory_ids.canonical(value, f"{path} advisories.ignore entry") for value in ignored
    )
    if len(set(entries)) != len(entries):
        raise PolicyError(f"{path} contains duplicate advisory exceptions")
    return entries


def main(argv: list[str] | None = None) -> int:
    repo_root = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--audit-config", type=Path, default=repo_root / ".cargo/audit.toml")
    parser.add_argument("--deny-config", type=Path, default=repo_root / "deny.toml")
    parser.add_argument(
        "--registry",
        type=Path,
        default=repo_root / ".github/policies/rust-advisory-exceptions.json",
    )
    parser.add_argument(
        "--today",
        default=datetime.now(timezone.utc).date().isoformat(),
        help="UTC policy evaluation date in YYYY-MM-DD form",
    )
    arguments = parser.parse_args(argv)

    try:
        today = fields.iso_date(arguments.today, "--today")
        audit = set(ignored_advisories(arguments.audit_config))
        deny = set(ignored_advisories(arguments.deny_config))
        exceptions = advisories.load(arguments.registry, today)
    except (OSError, PolicyError) as error:
        print(f"Rust advisory policy failed closed: {error}", file=sys.stderr)
        return 2

    leased = {exception.advisory_id for exception in exceptions}
    if not (audit == deny == leased):
        print(
            "Rust advisory exceptions differ across audit, deny, and registry: "
            f"audit-not-deny={sorted(audit - deny)}, "
            f"deny-not-audit={sorted(deny - audit)}, "
            f"missing-from-registry={sorted((audit | deny) - leased)}, "
            f"missing-from-audit={sorted(leased - audit)}, "
            f"missing-from-deny={sorted(leased - deny)}",
            file=sys.stderr,
        )
        return 1

    claims = sorted(
        f"{exception.advisory_id}->{list(exception.workspaces)}"
        for exception in exceptions
    )
    print(
        f"Rust advisory exception policy synchronized ({len(leased)} leased entries, "
        f"evaluated {today.isoformat()}): {', '.join(claims)}."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
