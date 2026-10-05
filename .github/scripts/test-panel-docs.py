#!/usr/bin/env python3
"""Prove link and version mismatches fail the maintained-document guard."""

from __future__ import annotations

import importlib.util
import tempfile
from pathlib import Path

guard = Path(__file__).with_name("check-panel-docs.py")
spec = importlib.util.spec_from_file_location("panel_docs_guard", guard)
assert spec is not None and spec.loader is not None
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

with tempfile.TemporaryDirectory(prefix="pingora-panel-docs-") as directory:
    root = Path(directory)
    (root / "panel/gateway-pingora/src").mkdir(parents=True)
    (root / "docs/adr").mkdir(parents=True)
    (root / "panel/Cargo.toml").write_text(
        '[workspace]\nmembers = []\n[workspace.dependencies]\npingora-core = { version = "=0.9.0" }\n'
    )
    (root / "panel/gateway-pingora/src/lib.rs").write_text(
        'pub const PINGORA_PACKAGE_VERSION: &str = "0.9.0";\n'
    )
    (root / "README.md").write_text(
        '| **Pingora** | 0.9.0, kept in this repository. |\n[Panel](panel/README.md)\n'
    )
    (root / "README_ZH.md").write_text(
        '| **Pingora** | 0.9.0，随本仓库一起维护。 |\n[English](README.md)\n'
    )
    (root / "PRODUCT_SPEC.md").write_text('> 当前 Pingora crates：0.9.0\n')
    (root / "panel/README.md").write_text('[ADR](../docs/adr/decision.md)\n')
    (root / "docs/adr/decision.md").write_text('# Decision\n')
    (root / "docs/upstream-patches.md").write_text('[Specification](../PRODUCT_SPEC.md)\n')
    assert module.check(root) == [], module.check(root)

    readme = root / "README.md"
    readme.write_text(readme.read_text() + '[Missing](no-such-file.md)\n')
    assert any("broken link" in error for error in module.check(root))
    readme.write_text('| **Pingora** | 0.8.1, kept in this repository. |\n[Panel](panel/README.md)\n')
    assert any("README.md: current Pingora version" in error for error in module.check(root))
    readme.write_text('| **Pingora** | 0.9.0, kept here. |\nPingora 0.8.1\n')
    assert any("stale current Pingora version" in error for error in module.check(root))
    readme.write_text('| **Pingora** | 0.9.0, kept here. |\n')
    chinese = root / "README_ZH.md"
    chinese.write_text('| **Pingora** | 0.9.10，随本仓库一起维护。 |\n')
    assert any("README_ZH.md: current Pingora version" in error for error in module.check(root))
    chinese.write_text('| **Pingora** | 0.9.0，随本仓库一起维护。 |\n[Missing](no-such-file.md)\n')
    assert any("README_ZH.md: broken link" in error for error in module.check(root))
    chinese.write_text('| **Pingora** | 0.9.0，随本仓库一起维护。 |\n')
    (root / "docs/adr/decision.md").unlink()
    assert any("broken link" in error for error in module.check(root))

print("Panel documentation guard self-test passed.")
