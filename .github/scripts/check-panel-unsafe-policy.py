#!/usr/bin/env python3
"""Enforce unsafe isolation through compiler-level crate-root attributes.

Every workspace crate must lead with an active `#![forbid(unsafe_code)]`. A
crate that genuinely cannot is exempted by a lease in
`.github/policies/panel-unsafe-adapters.json`, which names an owner, a reason,
and a review date, and is bounded by a declared ceiling on how many such crates
may exist.

The exemption records only *who* is exempt and *why*. What an exempt crate must
prove instead lives in `ADAPTER_ATTRIBUTES` here, so a lease can never grant
itself a weaker substitute for the attribute it is escaping.
"""

from __future__ import annotations

import json
import subprocess
from dataclasses import dataclass
from datetime import date
from pathlib import Path

from policy import PolicyError, cli, fields, leases, registry

SAFE_ROOT_ATTRIBUTE = "#![forbid(unsafe_code)]"
ADAPTER_ATTRIBUTES = frozenset(
    {
        "#![forbid(clippy::undocumented_unsafe_blocks)]",
        "#![forbid(unsafe_op_in_unsafe_fn)]",
    }
)
DEFAULT_REGISTRY = ".github/policies/panel-unsafe-adapters.json"

@dataclass(frozen=True)
class AdapterPolicy:
    """Which crates may hold unsafe, and the ceiling on how many may."""

    max_adapters: int
    holders: dict[str, str]


DOCUMENTS: registry.DocumentRegistry[date, AdapterPolicy] = registry.DocumentRegistry(
    "unsafe adapter registry"
)


@DOCUMENTS.reader(1)
def _read_v1(document: dict, today: date) -> AdapterPolicy:
    """Read the v1 contract: one lease per crate exempt from forbidding unsafe."""
    if set(document) != {"schema_version", "max_adapters", "adapters"}:
        raise PolicyError(
            "unsafe adapter registry v1 must contain exactly schema_version, "
            "max_adapters, and adapters"
        )
    ceiling = document["max_adapters"]
    if not isinstance(ceiling, int) or isinstance(ceiling, bool) or not 1 <= ceiling <= 16:
        raise PolicyError("unsafe adapter max_adapters must be between 1 and 16")
    declared = document["adapters"]
    if not isinstance(declared, list) or not declared:
        raise PolicyError("unsafe adapter registry must hold at least one lease")

    holders: dict[str, str] = {}
    for index, entry in enumerate(declared):
        context = f"unsafe adapter lease[{index}]"
        mapping, accountability = leases.read_lease(
            entry, frozenset({"package"}), context, today
        )
        package = fields.crate_name(mapping["package"], f"{context} package")
        if package in holders:
            raise PolicyError(f"unsafe adapter registry leases {package} twice")
        holders[package] = accountability.owner
    if len(holders) > ceiling:
        raise PolicyError(
            f"unsafe adapter registry leases {len(holders)} crates, "
            f"exceeding its own {ceiling} ceiling"
        )
    return AdapterPolicy(max_adapters=ceiling, holders=holders)


def cargo_metadata(manifest: Path) -> dict[str, object]:
    if not manifest.is_file():
        raise PolicyError(f"Panel workspace manifest does not exist: {manifest}")
    try:
        completed = subprocess.run(
            [
                "cargo",
                "metadata",
                "--manifest-path",
                str(manifest),
                "--format-version",
                "1",
                "--no-deps",
                "--locked",
            ],
            check=True,
            capture_output=True,
            text=True,
        )
        return json.loads(completed.stdout)
    except (OSError, subprocess.CalledProcessError, json.JSONDecodeError) as error:
        detail = getattr(error, "stderr", "") or str(error)
        raise PolicyError(f"cannot read Cargo metadata: {detail.strip()}") from error


def target_roots(metadata: dict[str, object]) -> list[tuple[str, tuple[str, ...], Path]]:
    members = frozenset(metadata.get("workspace_members", []))
    roots: list[tuple[str, tuple[str, ...], Path]] = []
    packages = metadata.get("packages")
    if not isinstance(packages, list):
        raise PolicyError("Cargo metadata has no package list")
    for package in packages:
        if not isinstance(package, dict) or package.get("id") not in members:
            continue
        name = package.get("name")
        targets = package.get("targets")
        if not isinstance(name, str) or not isinstance(targets, list):
            raise PolicyError("Cargo metadata contains a malformed workspace package")
        for target in targets:
            if not isinstance(target, dict):
                raise PolicyError(f"Cargo metadata contains a malformed target for {name}")
            kinds = target.get("kind")
            source = target.get("src_path")
            if not isinstance(kinds, list) or not all(
                isinstance(kind, str) for kind in kinds
            ):
                raise PolicyError(f"Cargo target kind is malformed for {name}")
            if not isinstance(source, str):
                raise PolicyError(f"Cargo target path is malformed for {name}")
            roots.append((name, tuple(kinds), Path(source)))
    return roots


def leading_inner_attributes(contents: str) -> frozenset[str]:
    """Return exact, active crate attributes before the first source item or comment."""

    attributes: set[str] = set()
    for raw_line in contents.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        if line.startswith("#![") and line.endswith("]"):
            attributes.add(line)
            continue
        break
    return frozenset(attributes)


def violations(metadata: dict[str, object], policy: AdapterPolicy) -> list[str]:
    holders = policy.holders
    failures: list[str] = []
    exempt_seen: set[str] = set()
    for package, kinds, source in target_roots(metadata):
        try:
            contents = source.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as error:
            failures.append(f"{source}: cannot inspect crate root: {error}")
            continue
        attributes = leading_inner_attributes(contents)
        if package in holders and "lib" in kinds:
            exempt_seen.add(package)
            missing = sorted(ADAPTER_ATTRIBUTES.difference(attributes))
            if missing:
                failures.append(
                    f"{source}: unsafe adapter leased by {holders[package]} is "
                    f"missing attributes: {', '.join(missing)}"
                )
        elif SAFE_ROOT_ATTRIBUTE not in attributes:
            failures.append(
                f"{source}: crate target must lead with active {SAFE_ROOT_ATTRIBUTE}"
            )
    # A lease outliving its crate would silently pre-authorise unsafe in whatever
    # crate later takes that name.
    failures.extend(
        f"unsafe adapter lease held by {holders[package]} is stale; "
        f"the Panel workspace has no library crate named {package}"
        for package in sorted(set(holders) - exempt_seen)
    )
    return failures


def main(argv: list[str] | None = None) -> int:
    entry = cli.Entrypoint("Panel unsafe policy", __doc__)
    entry.parser.add_argument(
        "manifest", nargs="?", type=Path, default=Path("panel/Cargo.toml")
    )
    entry.add_registry(DEFAULT_REGISTRY)
    arguments = entry.parse(argv)

    try:
        policy = DOCUMENTS.load(arguments.registry, arguments.today)
        failures = violations(cargo_metadata(arguments.manifest), policy)
    except cli.FAILING as error:
        return entry.failed_closed(error)

    return entry.report(
        failures,
        f"Panel unsafe isolation policy verified "
        f"({len(policy.holders)} of {policy.max_adapters} permitted unsafe "
        f"adapters leased, evaluated {arguments.today.isoformat()})",
    )


if __name__ == "__main__":
    raise SystemExit(main())
