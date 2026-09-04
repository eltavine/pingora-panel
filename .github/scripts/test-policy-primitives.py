#!/usr/bin/env python3
"""Contract tests for the primitives every policy guard is built on.

The guards in this directory share `policy/` for field validation, the
owner/reason/expiry contract, schema-versioned loading, and installer option
grammars. A regression there silently weakens every guard at once, and the
guard-level self-tests only reach these paths incidentally. This file exercises
them directly.
"""

from __future__ import annotations

import contextlib
import io
import json
import tempfile
from datetime import date, timedelta
from pathlib import Path

from policy import (
    PolicyError,
    advisories,
    advisory_ids,
    cargo_deny,
    ci_yaml,
    cli,
    fields,
    finding_leases,
    installers,
    leases,
    owners,
    registry,
    workspaces,
)

TODAY = date(2026, 6, 1)


def rejects(scenario: str, call) -> None:
    try:
        call()
    except PolicyError:
        return
    raise AssertionError(f"{scenario} was accepted")


def accepts(scenario: str, call):
    try:
        return call()
    except PolicyError as error:
        raise AssertionError(f"{scenario} was rejected: {error}") from error


def check_dates() -> None:
    accepts("a canonical date", lambda: fields.iso_date("2026-12-31", "d"))
    # These parse through date.fromisoformat but are not the canonical spelling,
    # so only the round-trip comparison rejects them. Without it, an expiry could
    # be written in a form no reviewer reads as a date.
    for candidate in ("20261231", "2026-W01-1", "2026-366"):
        rejects(f"non-canonical date {candidate}", lambda c=candidate: fields.iso_date(c, "d"))
    for candidate in ("2026-6-31", "2026-02-30", "2026-13-01", "", " 2026-12-31", "tomorrow"):
        rejects(f"invalid date {candidate}", lambda c=candidate: fields.iso_date(c, "d"))


def check_text() -> None:
    accepts("ordinary text", lambda: fields.text("an owner", "t"))
    # Non-ASCII prose is ordinary, so the rule must not reach for a character
    # class it does not mean.
    accepts("accented latin", lambda: fields.text("r\u00e9sum\u00e9 of the exception", "t"))
    accepts("han characters", lambda: fields.text("\u4f9d\u8d56\u9879\u4f8b\u5916", "t"))
    for candidate in ("", "   ", " leading", "trailing ", 7, None):
        rejects(f"invalid text {candidate!r}", lambda c=candidate: fields.text(c, "t"))
    # A reviewer decides whether to accept an exception by reading it, so text
    # that renders differently from its source is refused. A bidirectional
    # override reorders the reason; an invisible formatting character deletes
    # words from the rendering while leaving them in the bytes.
    for label, candidate in (
        ("right-to-left override", "sensible\u202ereason"),
        ("left-to-right embedding", "a\u202ab"),
        ("first strong isolate", "a\u2068b"),
        ("zero-width space", "a\u200bb"),
        ("zero-width joiner", "a\u200db"),
        ("soft hyphen", "a\u00adb"),
        ("byte order mark", "a\ufeffb"),
        ("newline", "two\nlines"),
        ("carriage return", "two\rlines"),
        ("tab", "a\tb"),
        ("nul", "nul\0byte"),
        ("delete", "a\x7fb"),
    ):
        rejects(f"text containing a {label}", lambda c=candidate: fields.text(c, "t"))


def check_advisory_schemes() -> None:
    accepts(
        "a RUSTSEC identifier",
        lambda: advisory_ids.canonical("RUSTSEC-2026-0098"),
    )
    if advisory_ids.SCHEMES.accepted() != ("RUSTSEC",):
        raise AssertionError(
            f"accepted schemes changed to {advisory_ids.SCHEMES.accepted()} without "
            "a test saying so"
        )

    # A recognised-but-refused scheme has to be refused *with its reason*. Being
    # told a well-formed GHSA identifier is "malformed" sends a maintainer
    # looking for a typo instead of for the RUSTSEC cross-reference.
    for label, candidate, expected in (
        ("GHSA", "GHSA-q264-w97q-q778", "GHSA"),
        ("CVE", "CVE-2026-12345", "CVE"),
    ):
        try:
            advisory_ids.canonical(candidate)
        except PolicyError as error:
            if expected not in str(error) or "RUSTSEC" not in str(error):
                raise AssertionError(
                    f"refusing a {label} identifier did not explain the scope: {error}"
                ) from error
        else:
            raise AssertionError(f"a {label} identifier was accepted")

    for candidate in ("RUSTSEC-2026-98", "RUSTSEC-26-0098", "rustsec-2026-0098", "", 7):
        rejects(
            f"identifier {candidate!r}",
            lambda c=candidate: advisory_ids.canonical(c),
        )

    # Registering a scheme twice would leave which stance is in force depending
    # on import order.
    duplicate = advisory_ids.SchemeRegistry(advisory_ids.RUSTSEC)
    try:
        duplicate.register(advisory_ids.RUSTSEC)
    except ValueError:
        pass
    else:
        raise AssertionError("the same advisory scheme registered twice")

    # An empty registry must refuse everything rather than accept anything.
    rejects(
        "an identifier against a registry with no schemes",
        lambda: advisory_ids.SchemeRegistry().canonical("RUSTSEC-2026-0098"),
    )


def check_versions() -> None:
    assert fields.sorted_unique_versions(["1.0.109", "2.0.117"], "v") == ("1.0.109", "2.0.117")
    accepts("a prerelease version", lambda: fields.sorted_unique_versions(["1.0.0-rc.1"], "v"))

    # Versions are ordered by release, not as strings. String order puts 1.0.109
    # before 1.0.9, which made the order a maintainer would naturally write the
    # one the guard refused.
    for scenario, candidate in (
        ("numeric patch components", ["1.0.9", "1.0.109"]),
        ("numeric major components", ["2.0.0", "10.0.0"]),
        ("numeric minor components", ["1.9.0", "1.10.0"]),
        ("a prerelease below its release", ["1.0.0-rc.1", "1.0.0"]),
        ("numeric prerelease identifiers", ["1.0.0-alpha.2", "1.0.0-alpha.10"]),
        ("alphanumeric above numeric identifiers", ["1.0.0-1", "1.0.0-alpha"]),
    ):
        accepts(
            f"release order across {scenario}",
            lambda c=candidate: fields.sorted_unique_versions(c, "v"),
        )
    for scenario, candidate in (
        ("patch", ["1.0.109", "1.0.9"]),
        ("major", ["10.0.0", "2.0.0"]),
        ("prerelease placement", ["1.0.0", "1.0.0-rc.1"]),
    ):
        rejects(
            f"string order across {scenario}",
            lambda c=candidate: fields.sorted_unique_versions(c, "v"),
        )
    # The refusal names the order it wanted, or the author is left guessing.
    try:
        fields.sorted_unique_versions(["1.0.109", "1.0.9"], "v")
    except PolicyError as error:
        if "'1.0.9', '1.0.109'" not in str(error):
            raise AssertionError(f"refusal did not name the expected order: {error}")

    # The observed side has to use the same order, or a lease written correctly
    # would never match the finding it governs.
    finding = cargo_deny.crate_finding("duplicate")(
        {
            "graphs": [
                {"Krate": {"name": "libc", "version": "1.0.109"}},
                {"Krate": {"name": "libc", "version": "1.0.9"}},
            ]
        }
    )
    if finding.versions != ("1.0.9", "1.0.109"):
        raise AssertionError(
            f"cargo-deny findings are ordered differently from leases: {finding.versions}"
        )

    for candidate in (
        ["2.0.117", "1.0.109"],  # unsorted
        ["1.0.109", "1.0.109"],  # repeated
        [],  # empty
        ["1.0"],  # not a full triple
        ["^1.0.0"],  # a requirement, not a resolved version
        "1.0.0",  # not a list
    ):
        rejects(
            f"invalid version set {candidate!r}",
            lambda c=candidate: fields.sorted_unique_versions(c, "v"),
        )


def check_paths() -> None:
    assert fields.manifest_path("panel/Cargo.toml") == "panel/Cargo.toml"
    assert fields.manifest_path("Cargo.toml") == "Cargo.toml"
    for candidate in (
        "/panel/Cargo.toml",  # absolute
        "../Cargo.toml",  # escapes the repository
        "panel//Cargo.toml",  # non-canonical
        "panel\\Cargo.toml",  # Windows separator
        "panel/Cargo.lock",  # not a manifest
        "./Cargo.toml",  # non-canonical
    ):
        rejects(f"invalid manifest path {candidate}", lambda c=candidate: fields.manifest_path(c))


#: The accountability contract is tested against its own owners, so these cases
#: keep meaning what they mean if the real org chart changes.
FIXTURE_OWNERS = owners.OwnerRegistry(
    owners.Owner(name="team", accountable_for="the fixture")
)


def read_fixture_lease(entry, subject_fields=frozenset(), today=None):
    return leases.read_lease(
        entry, subject_fields, "l", today or TODAY, known_owners=FIXTURE_OWNERS
    )


def check_accountability() -> None:
    live = {"owner": "team", "reason": "because", "expires_on": "2026-06-01"}
    contract = accepts(
        "a lease expiring today",
        lambda: leases.Accountability.read(live, "l", known_owners=FIXTURE_OWNERS),
    )
    accepts(
        "a lease valid through its expiry",
        lambda: contract.require_reviewable("l", TODAY),
    )
    rejects(
        "a lease that expired yesterday",
        lambda: leases.Accountability.read(
            {**live, "expires_on": "2026-05-31"}, "l", known_owners=FIXTURE_OWNERS
        ).require_reviewable("l", TODAY),
    )

    # Expiry is only accountability while somebody will still be there to be
    # asked, so the horizon is enforced at both ends and on its exact boundary.
    horizon = TODAY + leases.REVIEW_HORIZON
    accepts(
        "a lease expiring on the last day of the review horizon",
        lambda: read_fixture_lease({**live, "expires_on": horizon.isoformat()}),
    )
    for label, expires_on in (
        ("one day past the horizon", (horizon + timedelta(days=1)).isoformat()),
        ("a lease outliving everyone", "2999-01-01"),
        ("the furthest date there is", "9999-12-31"),
    ):
        rejects(
            f"a lease expiring {label}",
            lambda e=expires_on: read_fixture_lease({**live, "expires_on": e}),
        )

    for missing in ("owner", "reason", "expires_on"):
        rejects(
            f"a lease missing {missing}",
            lambda m=missing: read_fixture_lease(
                {key: value for key, value in live.items() if key != m}
            ),
        )
    rejects(
        "a lease carrying an unknown field",
        lambda: read_fixture_lease({**live, "note": "x"}),
    )
    rejects(
        "a lease missing its subject field",
        lambda: read_fixture_lease(live, frozenset({"crate"})),
    )
    accepts(
        "a lease with exactly its subject and accountability fields",
        lambda: read_fixture_lease({**live, "crate": "syn"}, frozenset({"crate"})),
    )
    rejects("a lease that is not a mapping", lambda: read_fixture_lease([]))


def check_owner_registry() -> None:
    accepts("a registered owner", lambda: FIXTURE_OWNERS.require("team"))
    # A misspelling reads correctly in a diff and points at nobody, which is
    # the whole failure this registry exists to catch.
    for candidate in ("teem", "tea", "Team", "team ", "", None, 7):
        rejects(
            f"owner {candidate!r}",
            lambda c=candidate: FIXTURE_OWNERS.require(c),
        )
    # The refusal has to name who could have been meant, or it sends the author
    # looking for a typo in the wrong place.
    try:
        FIXTURE_OWNERS.require("teem")
    except PolicyError as error:
        if "team" not in str(error):
            raise AssertionError(f"refusal did not name the registered owners: {error}")

    duplicate = owners.OwnerRegistry(owners.PLATFORM)
    try:
        duplicate.register(owners.PLATFORM)
    except ValueError:
        pass
    else:
        raise AssertionError("the same owner registered twice")

    rejects(
        "any owner against an empty registry",
        lambda: owners.OwnerRegistry().require("team"),
    )
    # Every real owner carries what it answers for, so a lease naming one points
    # somewhere rather than at a bare string.
    for name in owners.REGISTERED.names():
        if not owners.REGISTERED.accountable_for(name):
            raise AssertionError(f"registered owner {name} says nothing it answers for")


def check_document_registry() -> None:
    documents: registry.DocumentRegistry[None, str] = registry.DocumentRegistry("fixture")

    @documents.reader(1)
    def _v1(document: dict, _context: None) -> str:
        return f"v1:{document.get('payload')}"

    @documents.reader(3)
    def _v3(document: dict, _context: None) -> str:
        return f"v3:{document.get('payload')}"

    assert documents.supported_versions() == (1, 3)

    try:
        documents.reader(1)(_v1)
    except ValueError:
        pass
    else:
        raise AssertionError("re-registering a schema version was accepted")

    try:
        documents.reader(0)(_v1)
    except ValueError:
        pass
    else:
        raise AssertionError("a schema version below one was accepted")

    with tempfile.TemporaryDirectory(prefix="policy-primitives.") as temporary:
        path = Path(temporary) / "document.json"

        def load(contents: object) -> str:
            path.write_text(json.dumps(contents), encoding="utf-8")
            return documents.load(path, None)

        assert load({"schema_version": 1, "payload": "a"}) == "v1:a"
        # An older reader keeps working after a newer schema is introduced.
        assert load({"schema_version": 3, "payload": "b"}) == "v3:b"
        for scenario, contents in (
            ("an unsupported schema version", {"schema_version": 2}),
            ("a document without a schema version", {"payload": "a"}),
            ("a boolean schema version", {"schema_version": True}),
            ("a string schema version", {"schema_version": "1"}),
            ("a document that is not an object", ["schema_version"]),
        ):
            rejects(scenario, lambda c=contents: load(c))

        path.write_text("{not json", encoding="utf-8")
        rejects("a document that is not JSON", lambda: documents.load(path, None))
        rejects(
            "a document that does not exist",
            lambda: documents.load(Path(temporary) / "absent.json", None),
        )


def check_installer_option_arity() -> None:
    def failures(command: str) -> list[str]:
        return installers.failures(ci_yaml.shell_tokens(f"run: {command}"))

    # A forbidden option that takes a value consumes it, so the value is not
    # mistaken for a requirement.
    assert failures("pip install --user --index-url https://example.invalid semgrep==1.2.3") == [
        "pip install source option is forbidden: --index-url"
    ]
    # A forbidden option that takes no value must not swallow the requirement,
    # which would report a second, misleading failure.
    assert failures("pip install --user --pre semgrep==1.2.3") == [
        "pip install option is forbidden: --pre"
    ]
    # An approved valued option is accepted in both spellings.
    assert failures("pip install --user --only-binary=:all: semgrep==1.2.3") == []
    assert failures("pip install --user --only-binary :all: semgrep==1.2.3") == []
    # An approved valued option must not absorb the token after it when that
    # token is itself an option. Absorbing it hid a forbidden index redirect as
    # the value of an approved option, and installers whose option values are not
    # themselves validated then accepted the redirect.
    assert failures(
        "pip install --user --only-binary --index-url=https://example.invalid semgrep==1.2.3"
    ) == [
        "pip install option requires a value: --only-binary",
        "pip install source option is forbidden: --index-url",
    ]
    assert (
        "cargo install source option is forbidden: --git"
        in failures("cargo install --locked --version --git=https://example.invalid pkg")
    )
    # The scan walks every token, so an installer reached indirectly is governed.
    assert failures("python3 -m pip install semgrep") != []
    assert failures("sudo pip install semgrep") != []


def check_yaml_comment_scanning() -> None:
    cases = {
        "cargo install x # comment": "cargo install x ",
        "cargo install x#notacomment": "cargo install x#notacomment",
        "echo '#' && cargo install x": "echo '#' && cargo install x",
        'echo "#" && cargo install x': 'echo "#" && cargo install x',
        r'echo "\"" # comment': r'echo "\"" ',
        "run: echo ok": "run: echo ok",
        # Outside quotes a backslash is literal in YAML, so it does not escape
        # the comment marker and must not hide the rest of the line.
        r"echo a\ # comment": "echo a\\ ",
    }
    for line, expected in cases.items():
        actual = ci_yaml.strip_comment(line)
        if actual != expected:
            raise AssertionError(f"strip_comment({line!r}) == {actual!r}, expected {expected!r}")

    embedded_hash = ci_yaml.shell_tokens("run: go install example.com/tool@v1.2.3#suffix")
    if embedded_hash != ["go", "install", "example.com/tool@v1.2.3#suffix"]:
        raise AssertionError(f"an embedded hash changed shell tokens: {embedded_hash}")

    folded = ci_yaml.run_blocks(
        "jobs:\n  check:\n    steps:\n      - run: >-\n          cargo\n          install tool\n"
    )
    if folded != ("cargo install tool",):
        raise AssertionError(f"folded run block was not normalized: {folded}")
    literal = ci_yaml.run_blocks(
        "jobs:\n  check:\n    steps:\n      - run: |\n          cargo\n          install tool\n"
    )
    if literal != ("cargo\ninstall tool",):
        raise AssertionError(f"literal run block lost its line breaks: {literal}")
    quoted_run = ci_yaml.run_blocks(
        "jobs:\n  check:\n    steps:\n      - 'run': cargo install tool\n"
    )
    if quoted_run != ("cargo install tool",):
        raise AssertionError(f"quoted run key disappeared: {quoted_run}")

    substitutions = ci_yaml.shell_substitutions(
        'echo "$(cargo install tool)" `go install example.com/tool@latest`'
    )
    if substitutions != (
        "cargo install tool",
        "go install example.com/tool@latest",
    ):
        raise AssertionError(f"shell substitutions were not extracted: {substitutions}")
    if ci_yaml.shell_substitutions("echo '$(cargo install inert)'"):
        raise AssertionError("single-quoted shell text was treated as executable")

    workflow = """paths:
  - .github/scripts/not-executed.py
steps:
  - run: python3 .github/scripts/inline.py # ignore.py
  - name: Block
    run: |
      python3 .github/scripts/block.py
      # python3 .github/scripts/commented.py
"""
    active = ci_yaml.active_run_text(workflow)
    if "not-executed.py" in active or "ignore.py" in active or "commented.py" in active:
        raise AssertionError(f"non-executable workflow text leaked into run source: {active!r}")
    if "inline.py" not in active or "block.py" not in active:
        raise AssertionError(f"executable workflow text disappeared: {active!r}")

    joined, split = ci_yaml.logical_lines("- run: cargo \\\n    install x\n")
    if not split:
        raise AssertionError("a tool name split from its subcommand was not reported")
    # A shell deletes backslash-newline rather than folding it to a space, so a
    # continuation inside a tool's name rejoins into that name. Folding to a
    # space instead reconstructed two harmless words and hid the invocation.
    joined, _ = ci_yaml.logical_lines("run: pi\\\np install --index-url=https://x pkg\n")
    if ci_yaml.shell_tokens(joined[0].code)[:2] != ["pip", "install"]:
        raise AssertionError(f"a continuation split a tool name: {joined[0].code!r}")
    if not installers.failures(ci_yaml.shell_tokens(joined[0].code)):
        raise AssertionError("an installer hidden by a continuation was accepted")

    joined, split = ci_yaml.logical_lines("- run: cargo install \\\n    x\n")
    if split or len(joined) != 1:
        raise AssertionError(f"an ordinary continuation was mishandled: {joined}, {split}")
    # The continuation keeps its indentation, which tokenizing collapses, so the
    # rejoined command reads as one argument list.
    if ci_yaml.shell_tokens(joined[0].code) != ["cargo", "install", "x"]:
        raise AssertionError(f"a rejoined continuation did not tokenize: {joined[0].code!r}")


def check_cargo_deny_report() -> None:
    kinds = cargo_deny.FindingKindRegistry(cargo_deny.DUPLICATE, cargo_deny.YANKED)
    checks = ("advisories", "bans")
    summary = json.dumps(
        {"type": "summary", "fields": {"advisories": {}, "bans": {}}}
    )

    def diagnostic(code: str, severity: str = "warning", crate: str = "syn") -> str:
        return json.dumps(
            {
                "type": "diagnostic",
                "fields": {
                    "code": code,
                    "severity": severity,
                    "graphs": [{"Krate": {"name": crate, "version": "1.0.109"}}],
                },
            }
        )

    def parse(*lines: str):
        return cargo_deny.parse_report(list(lines), kinds, checks)

    found = accepts("a report with one registered finding", lambda: parse(diagnostic("duplicate"), summary))
    assert [finding.code for finding in found] == ["duplicate"]

    # A code another guard owns is passed over rather than refused.
    accepts(
        "a code this guard does not interpret",
        lambda: parse(diagnostic("advisory-not-detected"), summary),
    )

    # cargo-deny warns and exits successfully when it cannot reach an index,
    # having reported no yanked crates because it looked for none. Reading that
    # as "nothing is yanked" is the failure this refusal prevents.
    rejects("an index failure", lambda: parse(diagnostic("index-failure"), summary))
    rejects(
        "an ignore naming an advisory in no database",
        lambda: parse(diagnostic("unknown-advisory"), summary),
    )
    # A finding kind nobody has reasoned about must not read as silence.
    rejects(
        "an unclassified warning",
        lambda: parse(diagnostic("wildcard"), summary),
    )
    rejects(
        "an unclassified error",
        lambda: parse(diagnostic("banned", severity="error"), summary),
    )
    rejects(
        "a diagnostic carrying no code",
        lambda: parse(diagnostic(None, severity="error"), summary),
    )
    # Commentary carries no policy weight, so it does not fail the run.
    accepts(
        "an unclassified note",
        lambda: parse(diagnostic("skipped", severity="note"), summary),
    )

    # Without the completion summary a cargo-deny that died early would be
    # indistinguishable from a clean tree.
    rejects("a report with no summary", lambda: parse(diagnostic("duplicate")))
    rejects(
        "a summary omitting a requested check",
        lambda: parse(json.dumps({"type": "summary", "fields": {"advisories": {}}})),
    )
    rejects(
        "a report with two summaries",
        lambda: parse(summary, summary),
    )
    # Ordinary human-readable output must not disturb the scan, but a line that
    # presents itself as JSON and is malformed is ambiguous and must fail closed.
    accepts(
        "a report interleaved with plain text",
        lambda: parse("warning: something", summary),
    )
    rejects("a malformed JSON report line", lambda: parse("{not json", summary))


def check_entrypoint_contract() -> None:
    """The 0/1/2 contract every guard reports through."""
    entry = cli.Entrypoint("fixture guard", "fixture")
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(
        io.StringIO()
    ):
        satisfied = entry.report([], "all well")
        violated = entry.report(["a violation"], "all well")
        unreachable = entry.failed_closed(PolicyError("unreadable"))
    if satisfied != 0:
        raise AssertionError("a satisfied policy did not report 0")
    if violated != 1:
        raise AssertionError("a violated policy did not report 1")
    if unreachable != 2:
        raise AssertionError("an unreachable verdict did not report 2")

    # PolicyError has to be in the set guards catch, or a guard that raised one
    # would exit through a traceback and be read as an ordinary violation.
    for failure in (PolicyError, OSError, UnicodeError):
        if failure not in cli.FAILING:
            raise AssertionError(f"{failure.__name__} is not caught as fail-closed")

    # --today is resolved through the same canonicality rule as any policy
    # field, and a malformed one exits 2 rather than defaulting to now.
    parsed = cli.Entrypoint("fixture guard").parse(["--today", "2026-06-01"])
    if parsed.today != date(2026, 6, 1):
        raise AssertionError(f"--today resolved to {parsed.today}")
    for candidate in ("20260601", "2026-6-1", "not-a-date"):
        try:
            with contextlib.redirect_stderr(io.StringIO()):
                cli.Entrypoint("fixture guard").parse(["--today", candidate])
        except SystemExit as exit_code:
            if exit_code.code != 2:
                raise AssertionError(
                    f"--today {candidate!r} exited {exit_code.code}, not 2"
                ) from exit_code
        else:
            raise AssertionError(f"--today {candidate!r} was accepted")

    undated = cli.Entrypoint("fixture guard", dated=False).parse([])
    if hasattr(undated, "today"):
        raise AssertionError("an undated guard was given a --today anyway")


def check_enforcement_modes() -> None:
    """A workspace cannot declare a strictness its lockfile does not support."""
    kinds = (
        finding_leases.LeasedKind(
            kind=cargo_deny.YANKED, identified_by_advisory=False
        ),
    )
    identity = {
        "enforcement": "identity",
        "yanked": [
            {
                "crate": "libc",
                "versions": ["0.2.100"],
                "owner": "pingora-panel-platform",
                "reason": "fixture",
                "expires_on": "2027-01-01",
            }
        ],
    }
    accepts(
        "identity enforcement on a committed lockfile",
        lambda: finding_leases.read("panel/Cargo.toml", identity, kinds, TODAY, True),
    )
    # The mode is matched against what Git reports, not against what the
    # document claims, so neither direction of the mismatch can pass.
    rejects(
        "identity enforcement on an untracked lockfile",
        lambda: finding_leases.read("Cargo.toml", identity, kinds, TODAY, False),
    )
    ceilings = {
        "enforcement": "ceilings",
        "ceilings": {
            "yanked": {
                "max_findings": 1,
                "owner": "pingora-panel-platform",
                "reason": "fixture",
                "expires_on": "2027-01-01",
            }
        },
    }
    accepts(
        "count ceilings on an untracked lockfile",
        lambda: finding_leases.read("Cargo.toml", ceilings, kinds, TODAY, False),
    )
    rejects(
        "count ceilings on a committed lockfile",
        lambda: finding_leases.read("panel/Cargo.toml", ceilings, kinds, TODAY, True),
    )
    for mode in ("strict", "", None, 1):
        rejects(
            f"enforcement mode {mode!r}",
            lambda m=mode: finding_leases.read(
                "Cargo.toml", {"enforcement": m}, kinds, TODAY, False
            ),
        )
    if finding_leases.supported() != ("ceilings", "identity"):
        raise AssertionError(
            f"enforcement modes changed to {finding_leases.supported()} without a "
            "test saying so"
        )


def check_advisory_registry() -> None:
    """An advisory exception must say which workspaces it is claimed to cover."""
    with tempfile.TemporaryDirectory(prefix="advisory-registry.") as temporary:
        path = Path(temporary) / "exceptions.json"

        def written(document: object) -> Path:
            path.write_text(json.dumps(document), encoding="utf-8")
            return path

        exception = {
            "advisory_id": "RUSTSEC-2026-0098",
            "scope": "fixture",
            "workspaces": ["panel/Cargo.toml"],
            "owner": "pingora-panel-security",
            "reason": "fixture",
            "expires_on": "2027-01-01",
        }
        loaded = accepts(
            "a scoped advisory exception",
            lambda: advisories.load(written({"schema_version": 2, "exceptions": [exception]}), TODAY),
        )
        if loaded[0].workspaces != ("panel/Cargo.toml",):
            raise AssertionError(f"workspace claim read as {loaded[0].workspaces}")

        # v1 carried no workspace claim, so an ignore leaked into every
        # workspace. Reading it as if it did would reinstate that.
        rejects(
            "a v1 advisory registry",
            lambda: advisories.load(
                written({"schema_version": 1, "exceptions": [exception]}), TODAY
            ),
        )
        for field in ("workspaces", "owner", "reason", "expires_on", "advisory_id"):
            incomplete = {key: value for key, value in exception.items() if key != field}
            rejects(
                f"an advisory exception with no {field}",
                lambda d=incomplete: advisories.load(
                    written({"schema_version": 2, "exceptions": [d]}), TODAY
                ),
            )
        rejects(
            "the same advisory leased twice",
            lambda: advisories.load(
                written({"schema_version": 2, "exceptions": [exception, exception]}),
                TODAY,
            ),
        )


def check_workspace_lockfiles() -> None:
    assert workspaces.lockfile_for("Cargo.toml") == "Cargo.lock"
    assert workspaces.lockfile_for("panel/Cargo.toml") == "panel/Cargo.lock"
    assert workspaces.lockfile_for("a/b/Cargo.toml") == "a/b/Cargo.lock"
    repo_root = Path(__file__).resolve().parents[2]
    # Read from Git rather than declared, and this repository is the fixture:
    # the Panel workspace commits its lockfile and the vendored tree does not.
    if workspaces.resolves_reproducibly(repo_root, "panel/Cargo.toml") is not True:
        raise AssertionError("panel/Cargo.lock is expected to be tracked by Git")
    if workspaces.resolves_reproducibly(repo_root, "Cargo.toml") is not False:
        raise AssertionError("the root Cargo.lock is expected to be untracked")
    with tempfile.TemporaryDirectory(prefix="policy-primitives.") as temporary:
        rejects(
            "reproducibility read outside a Git repository",
            lambda: workspaces.resolves_reproducibly(Path(temporary), "Cargo.toml"),
        )


def main() -> int:
    check_dates()
    check_text()
    check_advisory_schemes()
    check_versions()
    check_paths()
    check_accountability()
    check_owner_registry()
    check_document_registry()
    check_installer_option_arity()
    check_cargo_deny_report()
    check_yaml_comment_scanning()
    check_entrypoint_contract()
    check_enforcement_modes()
    check_advisory_registry()
    check_workspace_lockfiles()
    print("Policy primitive self-test passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
