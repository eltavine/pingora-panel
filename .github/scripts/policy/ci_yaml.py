"""Small, quote-aware extraction of shell declarations from CI YAML.

Workflow syntax and security are delegated to actionlint and zizmor. The
remaining policy guards need only the active ``run:`` source so they can enforce
repository-specific installer and toolchain rules on a standard-library-only
runner. Sharing this narrow extraction keeps their notion of comments,
continuations, and scalar folding identical without reimplementing a YAML
validator.
"""

from __future__ import annotations

import re
import shlex
from dataclasses import dataclass

from .errors import PolicyError

_CONTINUED_TOOL_COMMAND = re.compile(r"(?:^|\s)(?:cargo|go|pip|pip3|python3?)\s+\\\s*$")
_RUN_KEY = r'(?:run|"run"|\'run\')'
_RUN_PREFIX = re.compile(rf"(?:-\s*)?{_RUN_KEY}\s*:\s*(.*)$")
_RUN_DECLARATION = re.compile(rf"^(\s*)(?:-\s*)?{_RUN_KEY}\s*:\s*(.*)$")
_BLOCK_SCALAR = re.compile(r"[|>](?:[1-9][+-]?|[+-][1-9]?)?")
_FLOW_START = re.compile(
    r"^\s*(?:-\s*)?(?:(?:[\"']?[A-Za-z0-9_-]+[\"']?)\s*:\s*)?[\[{]"
)


def strip_comment(line: str) -> str:
    """Remove a trailing YAML comment, respecting single and double quoting."""
    single_quoted = False
    double_quoted = False
    escaped = False
    for index, character in enumerate(line):
        if escaped:
            escaped = False
            continue
        # YAML only honours backslash escapes inside double quotes; elsewhere a
        # backslash is literal, including the shell continuations this scanner
        # has to preserve.
        if character == "\\" and double_quoted:
            escaped = True
            continue
        if character == "'" and not double_quoted:
            single_quoted = not single_quoted
            continue
        if character == '"' and not single_quoted:
            double_quoted = not double_quoted
            continue
        if (
            character == "#"
            and not single_quoted
            and not double_quoted
            and (index == 0 or line[index - 1].isspace())
        ):
            return line[:index]
    return line


def _flow_run_key(line: str, initial_depth: int) -> tuple[bool, int]:
    """Report a ``run`` key inside a YAML flow collection.

    The policy parser intentionally supports a restricted block-style subset
    instead of attempting to implement YAML. Flow collections containing shell
    are therefore rejected rather than silently omitted. Quote tracking keeps
    command or display text such as ``name: "{ run: inert }"`` inert.
    """
    code = strip_comment(line)
    if initial_depth == 0 and _FLOW_START.match(code) is None:
        return False, 0
    depth = initial_depth
    index = 0
    single_quoted = False
    double_quoted = False
    while index < len(code):
        character = code[index]
        if not single_quoted and not double_quoted and depth > 0:
            match = re.match(r'(?:run|"run"|\'run\')\s*:', code[index:])
            if match is not None:
                return True, depth
        if character == "\\" and double_quoted:
            index += 2
            continue
        if character == "'" and not double_quoted:
            single_quoted = not single_quoted
            index += 1
            continue
        if character == '"' and not single_quoted:
            double_quoted = not double_quoted
            index += 1
            continue
        if single_quoted or double_quoted:
            index += 1
            continue
        if character in "[{":
            depth += 1
            index += 1
            continue
        if character in "]}":
            depth = max(0, depth - 1)
            index += 1
            continue
        index += 1
    return False, depth


@dataclass(frozen=True)
class LogicalLine:
    """One backslash-joined logical line, tagged with where it started."""

    number: int
    code: str


def run_blocks(text: str) -> tuple[str, ...]:
    """Return only shell source declared by workflow ``run`` keys.

    Keeping this extraction shared prevents an invocation listed only in a path
    filter or comment from being mistaken for executed code. Both ordinary
    ``run:`` mappings and compact ``- run:`` sequence entries are supported.
    Folded ``>`` scalars are normalized to the spaces the YAML loader supplies;
    otherwise splitting an installer or script invocation across folded lines
    would make the policy inspect a different command from the runner.
    """
    lines = text.splitlines()
    commands: list[str] = []
    index = 0
    flow_depth = 0
    while index < len(lines):
        flow_run, flow_depth = _flow_run_key(lines[index], flow_depth)
        if flow_run:
            raise PolicyError(
                "flow-style YAML containing run is unsupported; use a block-style run mapping"
            )
        match = _RUN_DECLARATION.match(lines[index])
        if match is None:
            index += 1
            continue

        run_indent = len(match.group(1))
        value = match.group(2).strip()
        if value.startswith(("&", "*", "!")):
            raise PolicyError(
                "YAML anchors, aliases, and tags are unsupported for run values"
            )
        scalar = _BLOCK_SCALAR.fullmatch(value)
        if scalar is None:
            commands.append(value)
            index += 1
            continue

        index += 1
        block: list[str] = []
        while index < len(lines):
            line = lines[index]
            if line.strip():
                indent = len(line) - len(line.lstrip())
                if indent <= run_indent:
                    break
            block.append(line)
            index += 1
        commands.append(_block_scalar_source(block, run_indent, value))
    return tuple(commands)


def _block_scalar_source(lines: list[str], parent_indent: int, header: str) -> str:
    """Normalize one literal or folded block scalar to executable shell text."""
    explicit_indent = next((int(character) for character in header if character.isdigit()), None)
    first_content_indent = next(
        (len(line) - len(line.lstrip()) for line in lines if line.strip()),
        parent_indent + 1,
    )
    content_indent = (
        parent_indent + explicit_indent
        if explicit_indent is not None
        else first_content_indent
    )
    if content_indent <= parent_indent:
        raise PolicyError("run block scalar content must be indented below its key")

    normalized: list[str] = []
    for line in lines:
        if not line.strip():
            normalized.append("")
            continue
        indent = len(line) - len(line.lstrip())
        if indent < content_indent:
            raise PolicyError("run block scalar contains inconsistent indentation")
        normalized.append(line[content_indent:])

    if header.startswith("|"):
        return "\n".join(normalized)
    if not normalized:
        return ""

    folded = normalized[0]
    for previous, current in zip(normalized, normalized[1:]):
        # YAML preserves a line break around blank or more-indented content and
        # folds an ordinary content-to-content break to one space.
        separator = (
            "\n"
            if not previous
            or not current
            or previous[0].isspace()
            or current[0].isspace()
            else " "
        )
        folded += separator + current
    return folded


def active_run_text(text: str) -> str:
    """Return comment-stripped, continuation-joined source from workflow runs."""
    active: list[str] = []
    for block in run_blocks(text):
        lines, _split_failures = logical_lines(block)
        active.extend(line.code for line in lines)
    return "\n".join(active)


def logical_lines(text: str) -> tuple[list[LogicalLine], list[str]]:
    """Join backslash continuations, reporting split tool invocations.

    A tool name separated from its subcommand by a line continuation would let a
    caller hide the subcommand from a line-oriented policy, so those are
    reported rather than silently reassembled.
    """
    joined: list[LogicalLine] = []
    split_commands: list[str] = []
    buffered = ""
    start_line = 1
    for line_number, raw_line in enumerate(text.splitlines(), start=1):
        code = strip_comment(raw_line)
        if _CONTINUED_TOOL_COMMAND.search(code):
            split_commands.append(
                f"line {line_number}: tool command and subcommand must remain on one line"
            )
        if not buffered:
            start_line = line_number
        stripped = code.rstrip()
        if stripped.endswith("\\"):
            # A shell removes backslash-newline entirely rather than folding it
            # to a space. Inserting one here would reconstruct a different
            # command than the runner executes, and a continuation placed inside
            # a tool's name would then read as two harmless words.
            buffered += stripped[:-1]
            continue
        joined.append(LogicalLine(start_line, buffered + code))
        buffered = ""
    if buffered:
        joined.append(LogicalLine(start_line, buffered))
    return joined, split_commands


def shell_text(code: str) -> str:
    """Reduce one YAML line to the shell text it would execute."""
    stripped = code.strip()
    match = _RUN_PREFIX.match(stripped)
    if match is not None:
        stripped = match.group(1).strip()
    if len(stripped) >= 2 and stripped[0] == stripped[-1] and stripped[0] in {'"', "'"}:
        stripped = stripped[1:-1]
    return stripped


def shell_tokens(code: str) -> list[str]:
    """Tokenize shell text, keeping operators visible as separate tokens."""
    lexer = shlex.shlex(shell_text(code), posix=True, punctuation_chars=";&|")
    lexer.whitespace_split = True
    # `logical_lines` has already applied YAML's quote-aware comment rules.
    # Letting shlex interpret comments again is both redundant and inaccurate:
    # it treats an embedded `#` in `name@v1.2.3#suffix` as a comment even though
    # POSIX shells pass that text as part of the argument. A policy scanner must
    # validate the same token the installer will actually receive.
    lexer.commenters = ""
    try:
        return list(lexer)
    except ValueError as error:
        raise PolicyError(f"cannot parse shell declaration: {error}") from error


def shell_substitutions(code: str) -> tuple[str, ...]:
    """Extract executable ``$(...)`` and backtick bodies from shell source.

    POSIX tokenization deliberately keeps the contents of a double-quoted
    substitution inside one token. Policy callers need the nested commands too,
    while text inside single quotes must remain inert.
    """
    substitutions: list[str] = []
    index = 0
    single_quoted = False
    double_quoted = False
    while index < len(code):
        character = code[index]
        if character == "\\" and not single_quoted:
            index += 2
            continue
        if character == "'" and not double_quoted:
            single_quoted = not single_quoted
            index += 1
            continue
        if character == '"' and not single_quoted:
            double_quoted = not double_quoted
            index += 1
            continue
        if single_quoted:
            index += 1
            continue
        if code.startswith("$(", index):
            body, index = _dollar_substitution(code, index + 2)
            substitutions.append(body)
            substitutions.extend(shell_substitutions(body))
            continue
        if character == "`":
            body, index = _backtick_substitution(code, index + 1)
            substitutions.append(body)
            substitutions.extend(shell_substitutions(body))
            continue
        index += 1
    return tuple(substitutions)


def _dollar_substitution(code: str, start: int) -> tuple[str, int]:
    depth = 1
    index = start
    single_quoted = False
    double_quoted = False
    while index < len(code):
        character = code[index]
        if character == "\\" and not single_quoted:
            index += 2
            continue
        if character == "'" and not double_quoted:
            single_quoted = not single_quoted
            index += 1
            continue
        if character == '"' and not single_quoted:
            double_quoted = not double_quoted
            index += 1
            continue
        if not single_quoted and not double_quoted:
            if code.startswith("$(", index):
                depth += 1
                index += 2
                continue
            if character == ")":
                depth -= 1
                if depth == 0:
                    return code[start:index], index + 1
        index += 1
    raise PolicyError("unterminated shell command substitution")


def _backtick_substitution(code: str, start: int) -> tuple[str, int]:
    index = start
    while index < len(code):
        if code[index] == "\\":
            index += 2
            continue
        if code[index] == "`":
            return code[start:index], index + 1
        index += 1
    raise PolicyError("unterminated shell backtick substitution")


__all__ = [
    "LogicalLine",
    "active_run_text",
    "logical_lines",
    "run_blocks",
    "shell_substitutions",
    "shell_text",
    "shell_tokens",
    "strip_comment",
]
