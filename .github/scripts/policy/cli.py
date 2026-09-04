"""The entrypoint contract every guard script presents.

Guards agree on three exit codes so a workflow, and a reviewer reading a failed
job, can tell the outcomes apart without interpreting prose:

* ``0`` — the policy holds.
* ``1`` — the policy is violated, and every violation is listed on stderr.
* ``2`` — the guard could not reach a verdict, and refused to report one.

Keeping the distinction is the point. A guard that reported its own crash as a
violation would teach reviewers to read exit 1 as noise, and one that reported a
crash as success would be worse than having no guard at all.

The common options live here too, so ``--today`` means the same thing, and
resolves the same way, in every registry-backed guard.
"""

from __future__ import annotations

import argparse
import sys
from datetime import date, datetime, timezone
from pathlib import Path
from typing import Sequence

from . import fields
from .errors import PolicyError

#: Failures that mean the guard could not reach a verdict. Declared once so no
#: guard silently narrows the set and turns an unreadable input into a pass.
FAILING: tuple[type[BaseException], ...] = (PolicyError, OSError, UnicodeError)

#: The repository this package was checked out into.
REPO_ROOT: Path = Path(__file__).resolve().parents[3]


class Entrypoint:
    """One guard's argument surface and exit-code reporting."""

    def __init__(self, name: str, description: str | None = None, *, dated: bool = True):
        self.name = name
        self.parser = argparse.ArgumentParser(description=description)
        if dated:
            self.parser.add_argument(
                "--today",
                default=datetime.now(timezone.utc).date().isoformat(),
                help="UTC policy evaluation date in YYYY-MM-DD form",
            )
        self._dated = dated

    def add_registry(self, default_relative: str, flag: str = "--registry") -> None:
        """Add a policy-document option defaulting to its committed location."""
        self.parser.add_argument(
            flag, type=Path, default=REPO_ROOT / default_relative
        )

    def add_repo_root(self) -> None:
        self.parser.add_argument("--repo-root", type=Path, default=REPO_ROOT)

    def parse(self, argv: Sequence[str] | None = None) -> argparse.Namespace:
        """Parse arguments, resolving ``--today`` into a date.

        A malformed argument exits 2 through ``argparse``, which is the same
        code an unreadable policy document produces: in both cases the guard
        never formed an opinion.
        """
        arguments = self.parser.parse_args(argv)
        if self._dated:
            try:
                arguments.today = fields.iso_date(arguments.today, "--today")
            except PolicyError as error:
                self.parser.error(str(error))
        return arguments

    def failed_closed(self, error: BaseException) -> int:
        print(f"{self.name} failed closed: {error}", file=sys.stderr)
        return 2

    def report(
        self, failures: Sequence[str], summary: str, *, header: str | None = None
    ) -> int:
        if failures:
            if header is not None:
                print(header, file=sys.stderr)
                print("\n".join(f"  {failure}" for failure in failures), file=sys.stderr)
            else:
                print("\n".join(failures), file=sys.stderr)
            return 1
        print(f"{summary}.")
        return 0


def today_utc() -> date:
    return datetime.now(timezone.utc).date()


__all__ = ["Entrypoint", "FAILING", "REPO_ROOT", "today_utc"]
