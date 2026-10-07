#!/usr/bin/env python3
"""Check that every public API operation is offered by ppanel and the console.

`panel/surfaces.json` names, for each operation of the OpenAPI document, the
command and the console route that offer it, or why the operation belongs to
some surfaces only (ADR 0045). The command line and the console check that
what is named exists; this guard checks that the declaration covers the
document exactly and that parity holds for every non-specific operation.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

METHODS = {"get", "put", "post", "delete", "patch", "head", "options"}
KEYS = {"cli", "console", "console_via", "specific"}
COMMAND = re.compile(r"[a-z][a-z0-9-]*( [a-z][a-z0-9-]*)*")
LEAST_REASON = 20


def operations(document: dict) -> set[str]:
    found: set[str] = set()
    for item in document.get("paths", {}).values():
        for method, operation in item.items():
            if method in METHODS and "operationId" in operation:
                found.add(operation["operationId"])
    return found


def check(document: dict, surfaces: dict) -> tuple[list[str], str]:
    errors: list[str] = []
    declared = surfaces.get("operations")
    if not isinstance(declared, dict):
        return ["surfaces.json has no operations object"], ""
    published = operations(document)
    for missing in sorted(published - declared.keys()):
        errors.append(f"{missing} is not declared: name its command and console route")
    for stale in sorted(declared.keys() - published):
        errors.append(f"{stale} is declared but the OpenAPI document has no such operation")

    specific = 0
    for operation, entry in sorted(declared.items()):
        if not isinstance(entry, dict):
            errors.append(f"{operation} is not an object")
            continue
        unknown = entry.keys() - KEYS
        if unknown:
            errors.append(f"{operation} has unknown keys {sorted(unknown)}")
        values = {key: entry[key] for key in KEYS if key in entry}
        for key, value in values.items():
            if not isinstance(value, str) or not value.strip():
                errors.append(f"{operation}.{key} must be a non-empty string")
        cli = values.get("cli")
        console = values.get("console")
        via = values.get("console_via")
        if isinstance(cli, str) and not COMMAND.fullmatch(cli):
            errors.append(f"{operation}.cli {cli!r} is not a ppanel command path")
        if isinstance(console, str) and not console.startswith("/"):
            errors.append(f"{operation}.console {console!r} is not a console route")
        if via is not None:
            if console is None:
                errors.append(f"{operation}.console_via needs the console route it is read on")
            if via == operation or via not in declared:
                errors.append(f"{operation}.console_via {via!r} names no other declared operation")
        reason = values.get("specific")
        if reason is not None:
            specific += 1
            if isinstance(reason, str) and len(reason.strip()) < LEAST_REASON:
                errors.append(f"{operation}.specific should say why, in a sentence")
        elif cli is None or console is None:
            errors.append(
                f"{operation} needs both a command and a console route, or a specific reason"
            )
    shared = len(declared) - specific
    summary = (
        f"{shared} of {shared} shared operations are offered by the API, ppanel and the "
        f"console; {specific} belong to some surfaces only."
    )
    return errors, summary


if __name__ == "__main__":
    root = Path(__file__).resolve().parents[2]
    document = json.loads(
        (root / "panel/panel-api/tests/fixtures/openapi.json").read_text(encoding="utf-8")
    )
    surfaces = json.loads((root / "panel/surfaces.json").read_text(encoding="utf-8"))
    failures, summary = check(document, surfaces)
    for failure in failures:
        print(failure, file=sys.stderr)
    if failures:
        sys.exit(1)
    print(f"Panel surface parity verified: {summary}")
