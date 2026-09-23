#!/usr/bin/env python3
"""Check maintained Markdown links and current Pingora version declarations."""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path
from urllib.parse import unquote, urlsplit

MAINTAINED_DOCS = ("README.md", "panel/README.md", "PRODUCT_SPEC.md")
LINK = re.compile(r"(?<!!)\[[^\]]+\]\(([^)]+)\)")
VERSION_MARKER = re.compile(r"^> 当前 Pingora crates：([0-9]+\.[0-9]+\.[0-9]+)$", re.M)
ADAPTER_VERSION = re.compile(
    r'^pub const PINGORA_PACKAGE_VERSION: &str = "([0-9]+\.[0-9]+\.[0-9]+)";$',
    re.M,
)


def check(root: Path) -> list[str]:
    errors: list[str] = []
    docs = [root / name for name in MAINTAINED_DOCS]
    docs.extend(sorted((root / "docs/adr").glob("*.md")))
    for doc in docs:
        if not doc.is_file():
            errors.append(f"missing maintained document: {doc.relative_to(root)}")
            continue
        for target in LINK.findall(doc.read_text(encoding="utf-8")):
            parsed = urlsplit(target)
            if parsed.scheme or parsed.netloc or target.startswith(("#", "/")):
                continue
            relative_path = unquote(parsed.path)
            if relative_path and not (doc.parent / relative_path).exists():
                errors.append(f"{doc.relative_to(root)}: broken link {target}")

    try:
        manifest = tomllib.loads((root / "panel/Cargo.toml").read_text(encoding="utf-8"))
        dependency = manifest["workspace"]["dependencies"]["pingora-core"]["version"]
        if not re.fullmatch(r"=[0-9]+\.[0-9]+\.[0-9]+", dependency):
            errors.append(f"Pingora dependency must use an exact version: {dependency}")
            return errors
        current = dependency[1:]
        adapter = (root / "panel/gateway-pingora/src/lib.rs").read_text(encoding="utf-8")
        match = ADAPTER_VERSION.search(adapter)
        if not match or match.group(1) != current:
            errors.append("Pingora adapter version does not match the workspace dependency")
        for name in ("README.md", "PRODUCT_SPEC.md"):
            contents = (root / name).read_text(encoding="utf-8")
            match = VERSION_MARKER.search(contents)
            if not match or match.group(1) != current:
                errors.append(f"{name}: current Pingora version must be {current}")
            # Historical validation describes an older commit; check current
            # prose before and after it without rewriting that record.
            if name == "PRODUCT_SPEC.md" and "Initial Foundation 历史验证基线" in contents:
                before, historical = contents.split("Initial Foundation 历史验证基线", 1)
                after = historical.split("### 3.2 目标仓库边界", 1)
                contents = before + (after[1] if len(after) == 2 else "")
            for version in re.findall(r"Pingora ([0-9]+\.[0-9]+\.[0-9]+)", contents):
                if version != current:
                    errors.append(f"{name}: stale current Pingora version {version}, expected {current}")
        spec_text = (root / "PRODUCT_SPEC.md").read_text(encoding="utf-8")
        for version in re.findall(r"\| pingora-v1 \| ([0-9]+\.[0-9]+\.[0-9]+) / exact commit \|", spec_text):
            if version != current:
                errors.append(f"PRODUCT_SPEC.md: compatibility matrix must use {current}")
    except (OSError, KeyError, tomllib.TOMLDecodeError) as error:
        errors.append(f"could not inspect Pingora version: {error}")
    return errors


if __name__ == "__main__":
    project_root = Path(__file__).resolve().parents[2] if len(sys.argv) == 1 else Path(sys.argv[1])
    failures = check(project_root)
    for failure in failures:
        print(failure, file=sys.stderr)
    if failures:
        sys.exit(1)
    print("Maintained documentation links and Pingora version verified.")
