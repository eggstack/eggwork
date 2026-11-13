#!/usr/bin/env python3
"""Keep ``docs/quickstart.md`` and ``scripts/quickstart.sh`` in agreement.

Operations M008b §5.4 requires two things of the quickstart:

* the document's **command blocks** must match the checked-in script, so the
  document cannot drift from what CI actually runs; and
* the document's **shown output** must match what the node reports — at
  minimum, the set of ``doctor`` check names the document shows must equal the
  set the node emits. The plan records that the document currently shows seven
  checks while ``crates/eggwork-server/src/operations.rs`` emits eight
  (including ``host-telemetry``); this comparison is what would have caught it.

The script marks the commands the document must show with ``# doc:begin`` and
``# doc:end`` regions, each preceded by a ``# step: <id>`` marker. Everything
outside those regions is the script's own plumbing and is not compared.

Commands are compared by *shape*, not by literal text: the document writes
``/path/to/repo`` and ``$ROOT`` where the script writes ``$REPO_ROOT`` and
``$ROOT``, and both describe the same command. A shape replaces every variable
reference and every path with ``<PATH>``, keeps the literal arguments, and
keeps the name of an environment-variable assignment. The comparison is
bidirectional: every command the script runs must be shown in the document,
and every command the document shows must be run by the script.

Run it standalone::

    python3 scripts/check_quickstart_sync.py              # command blocks
    python3 scripts/check_quickstart_sync.py --report     # the block/step map
    python3 scripts/check_quickstart_sync.py --doctor-report doctor.json
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DOCUMENT = ROOT / "docs" / "quickstart.md"
SCRIPT = ROOT / "scripts" / "quickstart.sh"

STEP_MARKER_RE = re.compile(r"^\s*#\s*step:\s*(\S+)\s*$")
DOC_BEGIN = "doc:begin"
DOC_END = "doc:end"

HEREDOC_RE = re.compile(r"<<-?\s*(?:'([^']+)'|\"([^\"]+)\"|([A-Za-z_][A-Za-z0-9_]*))")

ASSIGNMENT_RE = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)=(.*)$", re.S)


@dataclass(frozen=True)
class Command:
    """One command, its origin, and the shape it is compared by."""

    origin: str
    label: str
    text: str
    fingerprint: str


def strip_comment(line: str) -> str:
    """Remove a trailing ``#`` comment, respecting quotes."""
    out: list[str] = []
    quote: str | None = None
    index = 0
    while index < len(line):
        character = line[index]
        if quote is not None:
            out.append(character)
            if character == "\\" and quote == '"' and index + 1 < len(line):
                out.append(line[index + 1])
                index += 2
                continue
            if character == quote:
                quote = None
        elif character in "\"'":
            quote = character
            out.append(character)
        elif character == "#" and (not out or out[-1].isspace()):
            break
        else:
            out.append(character)
        index += 1
    return "".join(out).rstrip()


def tokenize(line: str) -> list[str]:
    """Split a command into whitespace-separated tokens, respecting quotes."""
    tokens: list[str] = []
    current: list[str] = []
    quote: str | None = None
    index = 0
    while index < len(line):
        character = line[index]
        if quote is not None:
            if character == "\\" and quote == '"' and index + 1 < len(line):
                current.append(character)
                current.append(line[index + 1])
                index += 2
                continue
            if character == quote:
                quote = None
            else:
                current.append(character)
        elif character in "\"'":
            quote = character
        elif character.isspace():
            if current:
                tokens.append("".join(current))
                current = []
        else:
            current.append(character)
        index += 1
    if current:
        tokens.append("".join(current))
    return tokens


def normalize_value(text: str) -> str:
    """Replace every variable reference and every path with ``<PATH>``."""
    text = re.sub(r"\$\{[^}]*\}", "<PATH>", text)
    text = re.sub(r"\$\([^)]*\)", "<PATH>", text)
    text = re.sub(r"\$[A-Za-z_][A-Za-z0-9_]*", "<PATH>", text)
    if "/" in text:
        return "<PATH>"
    return text


def normalize_token(token: str) -> str:
    """Normalise one token, keeping the name of an assignment."""
    match = ASSIGNMENT_RE.match(token)
    if match is not None:
        return f"{match.group(1)}={normalize_value(match.group(2))}"
    return normalize_value(token)


def fingerprint(tokens: list[str]) -> str:
    """The comparable shape of a command.

    A trailing ``&`` is dropped: backgrounding is a scheduling modifier, not a
    different command. The document's reader runs the node in a terminal and
    the script runs it in the background, and the two are the same command.
    """
    if tokens and tokens[-1] == "&":
        tokens = tokens[:-1]
    return " ".join(normalize_token(token) for token in tokens)


def body_fingerprint(lines: list[str]) -> str:
    """The comparable shape of a heredoc body."""
    shapes = []
    for line in lines:
        stripped = strip_comment(line)
        if not stripped.strip():
            continue
        shapes.append(fingerprint(tokenize(stripped)))
    return "heredoc: " + " | ".join(shapes)


def iter_commands(lines: list[str]) -> list[tuple[str, str | None]]:
    """Yield ``(command, heredoc body)`` pairs from a run of shell lines."""
    commands: list[tuple[str, str | None]] = []
    index = 0
    total = len(lines)
    while index < total:
        raw = lines[index]
        if not raw.strip() or raw.strip().startswith("#"):
            index += 1
            continue
        buffer = raw
        while buffer.rstrip().endswith("\\") and index + 1 < total:
            buffer = buffer.rstrip()[:-1] + " " + lines[index + 1]
            index += 1
        command = strip_comment(buffer).strip()
        body: list[str] | None = None
        heredoc = HEREDOC_RE.search(buffer)
        if heredoc is not None:
            terminator = heredoc.group(1) or heredoc.group(2) or heredoc.group(3)
            index += 1
            body = []
            while index < total and lines[index].strip() != terminator:
                body.append(lines[index])
                index += 1
        index += 1
        if command:
            commands.append((command, body))
    return commands


def script_commands(script_text: str) -> list[Command]:
    """The documented commands of the script, with the step that owns each."""
    lines = script_text.split("\n")
    commands: list[Command] = []
    step = "(unmarked)"
    index = 0
    total = len(lines)
    while index < total:
        line = lines[index]
        marker = STEP_MARKER_RE.match(line)
        if marker is not None:
            step = marker.group(1)
            index += 1
            continue
        if DOC_BEGIN in line:
            index += 1
            region: list[str] = []
            while index < total and DOC_END not in lines[index]:
                region.append(lines[index])
                index += 1
            for command, body in iter_commands(region):
                commands.append(
                    Command(
                        origin="script",
                        label=step,
                        text=command,
                        fingerprint=fingerprint(tokenize(command)),
                    )
                )
                if body is not None:
                    commands.append(
                        Command(
                            origin="script",
                            label=step,
                            text="<heredoc body>",
                            fingerprint=body_fingerprint(body),
                        )
                    )
            index += 1
            continue
        index += 1
    return commands


def fenced_blocks(markdown_text: str, language: str | None = None) -> list[str]:
    """The contents of every fenced code block, optionally by language."""
    blocks: list[str] = []
    inside = False
    current_language = ""
    body: list[str] = []
    for line in markdown_text.split("\n"):
        stripped = line.lstrip()
        if not inside and (stripped.startswith("```") or stripped.startswith("~~~")):
            inside = True
            current_language = stripped[3:].strip()
            body = []
            continue
        if inside:
            if stripped.startswith("```") or stripped.startswith("~~~"):
                inside = False
                if language is None or current_language == language:
                    blocks.append("\n".join(body))
                body = []
                continue
            body.append(line)
    return blocks


def document_commands(markdown_text: str) -> list[Command]:
    """The commands the document shows, with the block that shows each."""
    commands: list[Command] = []
    for number, block in enumerate(fenced_blocks(markdown_text, "bash"), start=1):
        for command, body in iter_commands(block.split("\n")):
            commands.append(
                Command(
                    origin="document",
                    label=f"block {number}",
                    text=command,
                    fingerprint=fingerprint(tokenize(command)),
                )
            )
            if body is not None:
                commands.append(
                    Command(
                        origin="document",
                        label=f"block {number}",
                        text="<heredoc body>",
                        fingerprint=body_fingerprint(body),
                    )
                )
    return commands


def doctor_names(payload: str) -> list[str]:
    """The ``doctor`` check names in a JSON payload, in order."""
    try:
        data = json.loads(payload)
    except json.JSONDecodeError:
        return re.findall(r'"name"\s*:\s*"([^"]+)"', payload)
    if isinstance(data, dict):
        checks = data.get("checks")
        if isinstance(checks, list):
            names = [
                check.get("name")
                for check in checks
                if isinstance(check, dict) and isinstance(check.get("name"), str)
            ]
            if names:
                return names
    return re.findall(r'"name"\s*:\s*"([^"]+)"', payload)


def document_doctor_names(markdown_text: str) -> list[str] | None:
    """The ``doctor`` check names the document shows, in order."""
    for block in fenced_blocks(markdown_text, "json"):
        if '"checks"' not in block:
            continue
        names = doctor_names(block)
        if names:
            return names
    return None


def compare_commands(
    script: list[Command], document: list[Command]
) -> tuple[list[str], dict[str, list[str]], dict[str, list[str]]]:
    """Compare the two command sets, returning failures and the block/step map."""
    failures: list[str] = []
    document_by_shape: dict[str, list[str]] = {}
    for command in document:
        document_by_shape.setdefault(command.fingerprint, []).append(command.label)
    script_by_shape: dict[str, list[str]] = {}
    for command in script:
        script_by_shape.setdefault(command.fingerprint, []).append(command.label)

    for command in script:
        if command.fingerprint not in document_by_shape:
            failures.append(
                f"scripts/quickstart.sh step {command.label} runs a command that "
                f"docs/quickstart.md does not show: {command.text} "
                f"[shape: {command.fingerprint}]"
            )
    for command in document:
        if command.fingerprint not in script_by_shape:
            failures.append(
                f"docs/quickstart.md {command.label} shows a command that "
                f"scripts/quickstart.sh does not run: {command.text} "
                f"[shape: {command.fingerprint}]"
            )
    return failures, script_by_shape, document_by_shape


def report_map(
    script: list[Command], document: list[Command]
) -> list[str]:
    """The document block ↔ script step map, one line per script step."""
    document_by_shape: dict[str, list[str]] = {}
    for command in document:
        document_by_shape.setdefault(command.fingerprint, []).append(command.label)
    lines: list[str] = []
    for step in sorted({command.label for command in script}):
        blocks: list[str] = []
        for command in script:
            if command.label != step:
                continue
            for label in document_by_shape.get(command.fingerprint, []):
                if label not in blocks:
                    blocks.append(label)
        shown = ", ".join(blocks) if blocks else "(no matching document block)"
        lines.append(f"script step {step} <-> document {shown}")
    return lines


def check_doctor_report(document_text: str, report_path: Path) -> list[str]:
    """Compare the document's shown doctor checks with the node's report."""
    failures: list[str] = []
    try:
        payload = report_path.read_text(encoding="utf-8")
    except OSError as error:
        return [f"doctor report could not be read: {error}"]

    shown = document_doctor_names(document_text)
    if shown is None:
        return [
            "docs/quickstart.md shows no doctor output block, so the shown "
            "output cannot be checked against the node's report"
        ]

    reported = doctor_names(payload)
    if not reported:
        return [
            f"the doctor report {report_path} names no checks; the node's report "
            "could not be compared with the document"
        ]

    shown_set = set(shown)
    reported_set = set(reported)
    missing = sorted(reported_set - shown_set)
    extra = sorted(shown_set - reported_set)
    if missing:
        failures.append(
            "the node reports doctor check(s) the document does not show: "
            + ", ".join(missing)
        )
    if extra:
        failures.append(
            "the document shows doctor check(s) the node does not report: "
            + ", ".join(extra)
        )
    if not missing and not extra and shown != reported:
        failures.append(
            "the document shows the same doctor checks in a different order "
            f"(document: {', '.join(shown)}; node: {', '.join(reported)})"
        )
    return failures


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Check that docs/quickstart.md and scripts/quickstart.sh agree, and "
            "that the document's shown doctor output matches the node's."
        ),
    )
    parser.add_argument(
        "--document",
        default=str(DOCUMENT),
        help="the quickstart document (default: docs/quickstart.md)",
    )
    parser.add_argument(
        "--script",
        default=str(SCRIPT),
        help="the quickstart script (default: scripts/quickstart.sh)",
    )
    parser.add_argument(
        "--doctor-report",
        default=None,
        help="a captured `eggworkd doctor` JSON report to compare with the "
        "document's shown output",
    )
    parser.add_argument(
        "--report",
        action="store_true",
        help="print the document block ↔ script step map and exit",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    document_path = Path(args.document)
    script_path = Path(args.script)

    try:
        document_text = document_path.read_text(encoding="utf-8")
        script_text = script_path.read_text(encoding="utf-8")
    except OSError as error:
        print(f"quickstart sync check failed: {error}", file=sys.stderr)
        return 1

    failures: list[str] = []

    if args.report:
        script = script_commands(script_text)
        document = document_commands(document_text)
        for line in report_map(script, document):
            print(line)
        return 0

    if args.doctor_report is not None:
        failures.extend(
            check_doctor_report(document_text, Path(args.doctor_report))
        )
    else:
        script = script_commands(script_text)
        document = document_commands(document_text)
        if not script:
            failures.append(
                "scripts/quickstart.sh declares no documented commands; the "
                "document and the script cannot be compared"
            )
        if not document:
            failures.append(
                "docs/quickstart.md shows no bash command block; the document "
                "and the script cannot be compared"
            )
        if not failures:
            command_failures, _script_map, _document_map = compare_commands(
                script, document
            )
            failures.extend(command_failures)

    if failures:
        print("quickstart sync check failed:", file=sys.stderr)
        for failure in failures:
            print(f"- {failure}", file=sys.stderr)
        print(
            f"quickstart sync check failed: {len(failures)} disagreement(s) "
            "between docs/quickstart.md and scripts/quickstart.sh",
            file=sys.stderr,
        )
        return 1

    print(
        "quickstart sync check passed: the document's command blocks and shown "
        "doctor output agree with scripts/quickstart.sh"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
