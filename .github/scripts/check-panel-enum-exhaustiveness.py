#!/usr/bin/env python3
"""Require every public enum to have a recorded exhaustiveness decision.

Adding a variant to a public enum is a breaking change unless the enum is
`#[non_exhaustive]`. A mixed workspace without an explicit decision record is
what drift looks like: the convention exists, but whether a given enum can grow
is otherwise decided by whoever typed it and recorded nowhere.

Both answers are legitimate and the choice is not stylistic:

* an error or status type should be `#[non_exhaustive]`, because a caller has
  no business breaking when a new failure is described;
* an enum translated across a crate boundary onto a wire format should stay
  exhaustive, because the exhaustive match *is* the protection — a new variant
  must fail to compile in the encoder rather than be silently dropped or
  mapped onto something it is not.

So this guard does not pick a side. It requires that a side was picked: a
public enum either carries the attribute, or is named in the decision record
with an owner and a reason.

The record holds no expiry, unlike every lease in this repository. A lease
expires because a tolerated exception accumulates risk while nobody is looking.
A deliberately exhaustive enum accumulates nothing: the property is re-proved by
the compiler on every build, and the decision is re-read here on every run. What
would a review date add — a date to re-affirm that the wire encoder should still
fail to compile on an unhandled variant?
"""

from __future__ import annotations

import re
import tomllib
from dataclasses import dataclass
from pathlib import Path

from policy import PolicyError, cli, fields, owners, registry

#: A public enum, with any attributes immediately preceding it. Attributes are
#: captured as a block so `#[non_exhaustive]` is found wherever in the block it
#: sits, rather than only directly above the declaration.
PUBLIC_ENUM = re.compile(
    r"^(?P<attributes>(?:[ \t]*#\[[^\n]*\][ \t]*\n)*)[ \t]*pub enum[ \t]+(?P<name>\w+)",
    re.MULTILINE,
)
NON_EXHAUSTIVE_ATTRIBUTE = re.compile(r"#\[[ \t]*non_exhaustive[ \t]*\]")

@dataclass(frozen=True)
class Decision:
    """One public enum recorded as deliberately exhaustive."""

    package: str
    name: str
    owner: str
    reason: str

    @property
    def key(self) -> tuple[str, str]:
        return (self.package, self.name)


DOCUMENTS: registry.DocumentRegistry[None, dict[tuple[str, str], Decision]] = (
    registry.DocumentRegistry("panel exhaustive enum policy")
)


@DOCUMENTS.reader(1)
def _read_v1(document: dict, _context: None) -> dict[tuple[str, str], Decision]:
    if set(document) != {"schema_version", "exhaustive_enums"}:
        raise PolicyError(
            "panel exhaustive enum policy v1 must contain exactly schema_version "
            "and exhaustive_enums"
        )
    entries = document["exhaustive_enums"]
    if not isinstance(entries, list):
        raise PolicyError("exhaustive_enums must be a list")
    decisions: dict[tuple[str, str], Decision] = {}
    for index, entry in enumerate(entries):
        context = f"exhaustive_enums[{index}]"
        if not isinstance(entry, dict) or set(entry) != {
            "package",
            "name",
            "owner",
            "reason",
        }:
            raise PolicyError(
                f"{context} must contain exactly package, name, owner, and reason"
            )
        decision = Decision(
            package=fields.crate_name(entry["package"], f"{context} package"),
            name=fields.matching(entry["name"], f"{context} name", re.compile(r"\w+")),
            owner=owners.REGISTERED.require(entry["owner"], f"{context} owner"),
            reason=fields.text(entry["reason"], f"{context} reason"),
        )
        if decision.key in decisions:
            raise PolicyError(f"{context} repeats an earlier decision: {decision.key}")
        decisions[decision.key] = decision
    return decisions


def package_name(manifest: Path) -> str:
    try:
        document = tomllib.loads(manifest.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as error:
        raise PolicyError(f"cannot read {manifest}: {error}") from error
    name = document.get("package", {}).get("name")
    if not isinstance(name, str) or not name:
        raise PolicyError(f"{manifest} declares no package name")
    return name


def public_enums(package: str, source: Path) -> list[tuple[str, str, bool]]:
    """Report `(package, enum, is_non_exhaustive)` for one source file."""
    try:
        text = source.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as error:
        raise PolicyError(f"cannot read {source}: {error}") from error
    # Scan test modules conservatively too. A regex-only source guard cannot
    # safely prove where an arbitrarily nested test module ends; truncating at
    # its start would let a production declaration placed later in the file
    # escape policy. Test-only declarations do not need public visibility, so
    # refusing a bare `pub enum` there is the fail-closed tradeoff.
    body = text
    return [
        (
            package,
            match.group("name"),
            NON_EXHAUSTIVE_ATTRIBUTE.search(match.group("attributes")) is not None,
        )
        for match in PUBLIC_ENUM.finditer(body)
    ]


def violations(workspace: Path, decisions: dict[tuple[str, str], Decision]) -> list[str]:
    manifests = sorted(workspace.glob("*/Cargo.toml"))
    if not manifests:
        raise PolicyError(f"no member manifests under {workspace}")

    found: dict[tuple[str, str], bool] = {}
    for manifest in manifests:
        package = package_name(manifest)
        for source in sorted((manifest.parent / "src").rglob("*.rs")):
            for name, enum, non_exhaustive in public_enums(package, source):
                key = (name, enum)
                if key in found:
                    raise PolicyError(
                        f"{package} declares multiple public enums named {enum}; "
                        "the policy schema needs a module-qualified identity before "
                        "it can decide them safely"
                    )
                found[key] = non_exhaustive
    if not found:
        raise PolicyError(f"no public enums found under {workspace}")

    failures = [
        f"{package}::{enum} is a public enum with no exhaustiveness decision: "
        "add #[non_exhaustive] so a new variant is not a breaking change, or "
        "record why it must stay exhaustive"
        for (package, enum), non_exhaustive in sorted(found.items())
        if not non_exhaustive and (package, enum) not in decisions
    ]
    failures.extend(
        f"{package}::{enum} is recorded as deliberately exhaustive but carries "
        "#[non_exhaustive]; the record contradicts the code"
        for (package, enum), non_exhaustive in sorted(found.items())
        if non_exhaustive and (package, enum) in decisions
    )
    failures.extend(
        f"{package}::{enum} is recorded as deliberately exhaustive but no longer "
        "exists as a public enum"
        for package, enum in sorted(set(decisions) - set(found))
    )
    return failures


def main(argv: list[str] | None = None) -> int:
    entry = cli.Entrypoint("Panel enum exhaustiveness", __doc__, dated=False)
    entry.parser.add_argument("workspace", nargs="?", type=Path)
    entry.add_registry(".github/policies/panel-exhaustive-enums.json")
    arguments = entry.parse(argv)

    try:
        workspace = (arguments.workspace or cli.REPO_ROOT / "panel").resolve()
        decided = DOCUMENTS.load(arguments.registry, None)
        failures = violations(workspace, decided)
    except cli.FAILING as error:
        return entry.failed_closed(error)

    return entry.report(
        failures,
        "Every public enum has an exhaustiveness decision "
        f"({len(decided)} recorded as deliberately exhaustive)",
    )


if __name__ == "__main__":
    raise SystemExit(main())
