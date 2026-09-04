#!/usr/bin/env python3
"""Contract tests for the shared cargo-deny report.

The report is the single input both dependency guards read, so the property
under test is that an unusable one is refused where it is produced. A report
that reached neither guard is easy to reason about; one that reached both while
meaning nothing is not.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPT_DIRECTORY = Path(__file__).resolve().parent
EMITTER = SCRIPT_DIRECTORY / "emit-cargo-deny-report.py"

SUMMARY = json.dumps(
    {
        "type": "summary",
        "fields": {
            "advisories": {"warnings": 0},
            "bans": {"warnings": 0},
        },
    }
)


def diagnostic(code: str, severity: str = "warning", **fields: object) -> str:
    return json.dumps(
        {
            "type": "diagnostic",
            "fields": {"code": code, "severity": severity, **fields},
        }
    )


DUPLICATE = diagnostic(
    "duplicate",
    graphs=[
        {"Krate": {"name": "libc", "version": "0.2.100"}},
        {"Krate": {"name": "libc", "version": "0.2.101"}},
    ],
)


def write_fake_cargo(directory: Path) -> Path:
    """A stand-in `cargo` that replays whatever report the scenario asks for."""
    binaries = directory / "bin"
    binaries.mkdir(parents=True, exist_ok=True)
    fake = binaries / "cargo"
    fake.write_text(
        """#!/usr/bin/env python3
import os
import sys

if sys.argv[1:2] != ["deny"]:
    raise SystemExit(f"fake cargo received {sys.argv[1:]}")
sys.stderr.write(os.environ["FAKE_DENY_REPORT"])
raise SystemExit(int(os.environ.get("FAKE_DENY_STATUS", "0")))
""",
        encoding="utf-8",
    )
    fake.chmod(0o755)
    return binaries


def emit(
    binaries: Path,
    report: str,
    destination: Path,
    status: str = "0",
    *,
    exclusive: bool = False,
) -> int:
    """Run the emitter with `binaries` on PATH.

    `exclusive` replaces PATH rather than prepending to it, which is the only
    way to test an absent cargo: prepending leaves the real one discoverable
    further along, and the scenario would silently test nothing.
    """
    environment = os.environ.copy()
    environment["PATH"] = (
        str(binaries)
        if exclusive
        else f"{binaries}{os.pathsep}{environment['PATH']}"
    )
    environment["FAKE_DENY_REPORT"] = report
    environment["FAKE_DENY_STATUS"] = status
    return subprocess.run(
        [sys.executable, str(EMITTER), "panel/Cargo.toml", str(destination)],
        env=environment,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    ).returncode


def expect(actual: int, expected: int, scenario: str) -> None:
    if actual != expected:
        raise AssertionError(f"{scenario}: expected exit {expected}, got {actual}")


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="cargo-deny-report.") as temporary:
        root = Path(temporary)
        binaries = write_fake_cargo(root)

        destination = root / "nested" / "report.jsonl"
        expect(
            emit(binaries, "\n".join([DUPLICATE, SUMMARY]) + "\n", destination),
            0,
            "a complete report",
        )
        if not destination.is_file():
            raise AssertionError("a complete report was not written")
        replayed = destination.read_text(encoding="utf-8")
        if DUPLICATE not in replayed or SUMMARY not in replayed:
            raise AssertionError("the written report lost lines cargo-deny emitted")

        # cargo-deny fails the build for its own blocking findings in a separate
        # step, so a non-zero status still has to yield a readable report.
        expect(
            emit(
                binaries,
                "\n".join([DUPLICATE, SUMMARY]) + "\n",
                root / "failing.jsonl",
                status="1",
            ),
            0,
            "a complete report from a failing run",
        )

        # No summary means cargo-deny died before evaluating the graph. Both
        # guards read silence as success, so this must never become a report.
        truncated = root / "truncated.jsonl"
        expect(emit(binaries, DUPLICATE + "\n", truncated), 2, "a truncated report")
        if truncated.exists():
            raise AssertionError("a truncated report was written anyway")

        expect(
            emit(binaries, SUMMARY + "\n" + SUMMARY + "\n", root / "twice.jsonl"),
            2,
            "a report claiming to complete twice",
        )

        expect(
            emit(binaries, "{not-json}\n" + SUMMARY + "\n", root / "malformed.jsonl"),
            2,
            "a malformed JSON report line",
        )

        malformed_diagnostic = json.dumps(
            {"type": "diagnostic", "fields": ["not", "a", "mapping"]}
        )
        expect(
            emit(
                binaries,
                malformed_diagnostic + "\n" + SUMMARY + "\n",
                root / "malformed-diagnostic.jsonl",
            ),
            2,
            "a diagnostic whose fields changed shape",
        )

        expect(
            emit(
                binaries,
                diagnostic("", "warning") + "\n" + SUMMARY + "\n",
                root / "missing-code.jsonl",
            ),
            2,
            "a diagnostic without an identity",
        )

        expect(
            emit(
                binaries,
                diagnostic("duplicate", "brand-new-severity") + "\n" + SUMMARY + "\n",
                root / "unknown-severity.jsonl",
            ),
            2,
            "a diagnostic carrying an unknown severity",
        )

        # A summary covering only some checks means a guard reading it would
        # draw conclusions about a check that never ran.
        partial = json.dumps(
            {"type": "summary", "fields": {"advisories": {"warnings": 0}}}
        )
        expect(
            emit(binaries, partial + "\n", root / "partial.jsonl"),
            2,
            "a summary omitting the bans check",
        )

        malformed_summary = json.dumps(
            {
                "type": "summary",
                "fields": {"advisories": {"warnings": 0}, "bans": None},
            }
        )
        expect(
            emit(binaries, malformed_summary + "\n", root / "bad-summary.jsonl"),
            2,
            "a summary whose check counters changed shape",
        )

        expect(
            emit(
                binaries,
                "\n".join([DUPLICATE, SUMMARY]) + "\n",
                root / "unexpected-status.jsonl",
                status="2",
            ),
            2,
            "an unexpected cargo-deny process status",
        )

        # An index failure makes the report's silence about yanked crates
        # meaningless while cargo-deny still exits successfully.
        expect(
            emit(
                binaries,
                "\n".join([diagnostic("index-failure"), SUMMARY]) + "\n",
                root / "index.jsonl",
            ),
            2,
            "a report produced without a registry index",
        )

        # A diagnostic nobody has classified must be refused here rather than
        # skipped by whichever guard happens not to recognise it.
        expect(
            emit(
                binaries,
                "\n".join([diagnostic("brand-new-check"), SUMMARY]) + "\n",
                root / "unclassified.jsonl",
            ),
            2,
            "a report carrying an unclassified diagnostic",
        )

        # Commentary carries no policy weight, so an unknown note is passed over.
        expect(
            emit(
                binaries,
                "\n".join([diagnostic("brand-new-note", "note"), SUMMARY]) + "\n",
                root / "note.jsonl",
            ),
            0,
            "a report carrying an unclassified note",
        )

        missing = root / "absent-bin"
        missing.mkdir()
        expect(
            emit(missing, SUMMARY + "\n", root / "nocargo.jsonl", exclusive=True),
            2,
            "no cargo on PATH",
        )

    print("Shared cargo-deny report self-test passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
