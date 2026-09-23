#!/usr/bin/env python3
"""Guard Eggwork child-process ownership and crate dependency direction."""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# These are ownership boundaries, not filename-based exceptions. A new owner
# requires an architecture review and a named entry here with its reason.
APPROVED_PROCESS_OWNERS = {
    "eggwork-runner": "canonical finite-process lifecycle and Linux resource wrappers",
    "eggwork-sandbox-helper": "target/helper mechanics for enforced sandbox execution",
}

FORBIDDEN_PROCESS_DEPS = {
    "command-group",
    "duct",
    "process-wrap",
    "portable-pty",
    "subprocess",
}


def strip_comments_and_literals(source: str) -> str:
    """Blank Rust comments and literals while preserving newlines/positions."""
    out = list(source)
    i = 0
    n = len(source)
    while i < n:
        if source.startswith("//", i):
            end = source.find("\n", i)
            end = n if end < 0 else end
            for j in range(i, end):
                out[j] = " "
            i = end
            continue
        if source.startswith("/*", i):
            depth = 1
            j = i + 2
            while j < n and depth:
                if source.startswith("/*", j):
                    depth += 1
                    j += 2
                elif source.startswith("*/", j):
                    depth -= 1
                    j += 2
                else:
                    j += 1
            for k in range(i, j):
                if out[k] != "\n":
                    out[k] = " "
            i = j
            continue

        # Raw strings may contain quotes and comment delimiters.
        raw = re.match(r'(?:b)?r(#+)?"', source[i:])
        if raw:
            hashes = raw.group(1) or ""
            start = i
            j = i + raw.end()
            terminator = '"' + hashes
            end = source.find(terminator, j)
            i = n if end < 0 else end + len(terminator)
            for k in range(start, i):
                if out[k] != "\n":
                    out[k] = " "
            continue

        # Normal/byte strings and character literals.
        prefix = 1 if source[i] == "b" and i + 1 < n and source[i + 1] in "\"'" else 0
        quote_i = i + prefix
        if quote_i < n and source[quote_i] in "\"'":
            quote = source[quote_i]
            if quote == "'" and quote_i + 1 < n and source[quote_i + 1].isalpha():
                i += 1  # Rust lifetime, not a character literal.
                continue
            j = quote_i + 1
            while j < n:
                if source[j] == "\\":
                    j += 2
                elif source[j] == quote:
                    j += 1
                    break
                else:
                    j += 1
            for k in range(i, min(j, n)):
                if out[k] != "\n":
                    out[k] = " "
            i = j
            continue
        i += 1
    return "".join(out)


def process_spawn_patterns(source: str) -> list[str]:
    code = strip_comments_and_literals(source)
    patterns = [
        r"\b(?:std|tokio)\s*::\s*process\s*::\s*Command\b",
        r"\bprocess\s*::\s*Command\b",
        r"\b(?:spawn_process|spawn_child_process|exec_process)\s*\(",
    ]
    findings = [pattern for pattern in patterns if re.search(pattern, code)]

    process_imports = re.findall(
        r"\bprocess\s*::\s*(?:Command\b|\{([^}]*)\})", code
    )
    aliases = {"Command"} if process_imports else set()
    for group in process_imports:
        aliases.update(re.findall(r"\bCommand\s+as\s+([A-Za-z_][A-Za-z0-9_]*)", group))
    if any(re.search(rf"\b{alias}\s*::\s*new\s*\(", code) for alias in aliases):
        findings.append("imported process Command::new")
    return findings


def self_test() -> None:
    synthetic = """
        // This comment must not trigger: std::process::Command::new(\"x\");
        let docs = r#\"tokio::process::Command::new(\"x\")\"#;
        use std::process::Command;
        fn forbidden() { let _child = Command::new(\"tool\").spawn(); }
    """
    if not process_spawn_patterns(synthetic):
        raise RuntimeError("negative self-test failed to detect a forbidden spawn")
    safe = '// std::process::Command::new("x");\nlet docs = "Command::new(\\"x\\")";'
    if process_spawn_patterns(safe):
        raise RuntimeError("scanner self-test reported a comment/string as a spawn")


def crate_for_source(path: Path) -> str | None:
    try:
        rel = path.relative_to(ROOT)
    except ValueError:
        return None
    if len(rel.parts) >= 3 and rel.parts[0] == "crates":
        return rel.parts[1]
    return None


def scan_sources() -> tuple[list[str], dict[str, int]]:
    errors: list[str] = []
    approved_hits: dict[str, int] = {}
    for path in sorted((ROOT / "crates").glob("*/src/**/*.rs")):
        crate = crate_for_source(path)
        if crate is None:
            continue
        hits = process_spawn_patterns(path.read_text(encoding="utf-8"))
        if not hits:
            continue
        if crate in APPROVED_PROCESS_OWNERS:
            approved_hits[crate] = approved_hits.get(crate, 0) + len(hits)
        else:
            errors.append(
                f"{path.relative_to(ROOT)}: process creation outside approved owners ({', '.join(hits)})"
            )
    return errors, approved_hits


def check_dependencies() -> list[str]:
    result = subprocess.run(
        ["rtk", "cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    metadata = json.loads(result.stdout)
    packages = {package["name"]: package for package in metadata["packages"]}
    errors: list[str] = []

    def deps(name: str) -> dict[str, dict]:
        return {dep["name"]: dep for dep in packages[name]["dependencies"]}

    for crate in ("eggwork-core", "eggwork-client"):
        forbidden = {"eggwork-runner", "eggwork-server"}
        if crate == "eggwork-core":
            forbidden.add("eggwork-client")
        overlap = forbidden.intersection(deps(crate))
        if overlap:
            errors.append(f"{crate} must not depend on {', '.join(sorted(overlap))}")

    server_deps = deps("eggwork-server")
    tokio = server_deps.get("tokio")
    if tokio and "process" in tokio.get("features", []):
        errors.append("eggwork-server enables Tokio process creation; use eggwork-runner")
    extra = FORBIDDEN_PROCESS_DEPS.intersection(server_deps)
    if extra:
        errors.append(f"eggwork-server adds process-owner dependencies: {', '.join(sorted(extra))}")
    if "eggwork-runner" not in server_deps:
        errors.append("eggwork-server must depend on the canonical eggwork-runner")
    return errors


def main() -> int:
    try:
        self_test()
        source_errors, approved_hits = scan_sources()
        errors = source_errors + check_dependencies()
    except (OSError, RuntimeError, subprocess.CalledProcessError, json.JSONDecodeError) as error:
        print(f"execution ownership guard failed: {error}", file=sys.stderr)
        return 1
    if errors:
        print("execution ownership guard failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    owners = ", ".join(f"{name} ({count} spawn sites)" for name, count in sorted(approved_hits.items()))
    print(f"execution ownership guard passed; approved process owners: {owners or 'none'}")
    print("negative scanner self-test passed; crate dependency direction passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
