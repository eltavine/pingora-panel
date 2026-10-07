#!/usr/bin/env python3
"""Negative and positive contract cases for the surface parity guard."""

from __future__ import annotations

import copy
import importlib.util
from pathlib import Path

guard = Path(__file__).with_name("check-panel-surfaces.py")
spec = importlib.util.spec_from_file_location("surface_parity_guard", guard)
assert spec is not None and spec.loader is not None
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

document = {
    "paths": {
        "/api/v1/sites": {
            "get": {"operationId": "list_sites"},
            "post": {"operationId": "create_site"},
        },
        "/api/v1/sites/{id}": {"get": {"operationId": "get_site"}},
        "/api/v1/openapi.json": {"get": {"operationId": "openapi"}},
    }
}
valid = {
    "operations": {
        "list_sites": {"cli": "site list", "console": "/sites"},
        "create_site": {"cli": "site create", "console": "/sites"},
        "get_site": {"cli": "site show", "console": "/sites", "console_via": "list_sites"},
        "openapi": {"specific": "The API's own description, for clients and tools."},
    }
}


def errors(surfaces: dict) -> list[str]:
    return module.check(document, surfaces)[0]


assert errors(valid) == [], errors(valid)
assert "3 of 3 shared operations" in module.check(document, valid)[1]


def changed(edit) -> list[str]:
    surfaces = copy.deepcopy(valid)
    edit(surfaces["operations"])
    return errors(surfaces)


def fails(found: list[str], fragment: str) -> None:
    assert any(fragment in error for error in found), (fragment, found)


fails(changed(lambda ops: ops.pop("create_site")), "create_site is not declared")
fails(changed(lambda ops: ops.update(gone={"cli": "x", "console": "/x"})), "gone is declared")
fails(changed(lambda ops: ops["list_sites"].pop("console")), "needs both a command")
fails(changed(lambda ops: ops["create_site"].pop("cli")), "needs both a command")
fails(changed(lambda ops: ops["list_sites"].update(cli="Site List")), "not a ppanel command")
fails(changed(lambda ops: ops["list_sites"].update(console="sites")), "not a console route")
fails(changed(lambda ops: ops["get_site"].update(console_via="nowhere")), "names no other")
fails(changed(lambda ops: ops["get_site"].update(console_via="get_site")), "names no other")
fails(changed(lambda ops: ops["openapi"].update(specific="docs")), "should say why")
fails(changed(lambda ops: ops["list_sites"].update(surfaces="all")), "unknown keys")
fails(changed(lambda ops: ops["list_sites"].update(cli="")), "non-empty string")
assert module.check(document, {})[0] == ["surfaces.json has no operations object"]

print("Panel surface parity guard self-test passed.")
