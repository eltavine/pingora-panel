"""Whether a Cargo workspace resolves reproducibly.

Every policy that makes a claim about a resolved dependency graph depends on one
underlying fact: is the workspace's lockfile committed? A workspace with a
committed lockfile resolves identically on every run, so exact claims about it
hold. A workspace whose lockfile is untracked re-resolves on every run, and not
only the versions change — the set of duplicated crates and the set of matching
advisories change too, whenever any transitive dependency publishes.

The fact is read from Git rather than declared in a policy file, so no document
can claim a strictness the repository does not support.
"""

from __future__ import annotations

import subprocess
from pathlib import Path, PurePosixPath

from .errors import PolicyError


def lockfile_for(workspace: str) -> str:
    """Return the repository-relative lockfile path for one workspace manifest."""
    parent = PurePosixPath(workspace).parent
    return (parent / "Cargo.lock").as_posix()


def resolves_reproducibly(repo_root: Path, workspace: str) -> bool:
    """Report whether the workspace's lockfile is tracked by Git.

    Fails closed: a repository Git cannot describe is never assumed reproducible,
    it raises, so a guard cannot silently relax because Git was unavailable.
    """
    lockfile = lockfile_for(workspace)
    try:
        completed = subprocess.run(
            ["git", "-C", str(repo_root), "ls-files", "--error-unmatch", "--", lockfile],
            check=False,
            capture_output=True,
            text=True,
        )
    except OSError as error:
        raise PolicyError(f"cannot invoke Git to inspect {lockfile}: {error}") from error
    if completed.returncode == 0:
        return completed.stdout.strip() == lockfile
    # `--error-unmatch` exits non-zero for an untracked path, which is the
    # answer rather than an error. Any other failure means Git itself failed.
    inside = subprocess.run(
        ["git", "-C", str(repo_root), "rev-parse", "--is-inside-work-tree"],
        check=False,
        capture_output=True,
        text=True,
    )
    if inside.stdout.strip() != "true":
        raise PolicyError(f"not a Git repository: {repo_root}")
    return False


def describe(reproducible: bool) -> str:
    return "committed lockfile" if reproducible else "untracked lockfile"


__all__ = ["describe", "lockfile_for", "resolves_reproducibly"]
