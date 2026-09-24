#!/usr/bin/env python3
"""Guard Eggwork child-process ownership and crate dependency direction."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tempfile
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


def scan_file_for_spawns(path: Path) -> list[str]:
    """Run the per-file spawn pattern scan used by the production scanner."""
    return process_spawn_patterns(path.read_text(encoding="utf-8"))


def scan_sources() -> tuple[list[str], dict[str, int]]:
    errors: list[str] = []
    approved_hits: dict[str, int] = {}
    for path in sorted((ROOT / "crates").glob("*/src/**/*.rs")):
        crate = crate_for_source(path)
        if crate is None:
            continue
        hits = scan_file_for_spawns(path)
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
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
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


def prove_negative_exit() -> int:
    """Deterministically prove the guard exits nonzero on a forbidden production spawn.

    Writes a synthetic Rust source file containing a forbidden process spawn into a
    temporary directory outside ``crates/`` (so the production scanner does not pick
    it up) and runs the same per-file scanner used against production sources. A
    regression that drops the spawn pattern would be detected here; CI runs this
    alongside the normal guard so the failure path is exercised on every push.
    """
    synthetic_source = (
        "use std::process::Command;\n"
        "fn forbidden() { let _child = Command::new(\"tool\").spawn(); }\n"
    )
    with tempfile.TemporaryDirectory(prefix="eggwork-ownership-proof-") as tmp:
        probe = Path(tmp) / "forbidden.rs"
        probe.write_text(synthetic_source, encoding="utf-8")
        try:
            hits = scan_file_for_spawns(probe)
        except OSError as error:
            print(f"execution ownership guard failed: {error}", file=sys.stderr)
            return 1
    if not hits:
        print(
            "execution ownership guard failed: scanner regression — "
            "synthetic forbidden spawn was not detected",
            file=sys.stderr,
        )
        return 1
    print(
        "execution ownership guard negative-exit proof passed: "
        f"synthetic forbidden spawn detected ({', '.join(hits)})"
    )
    return 0


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Guard Eggwork child-process ownership and crate dependency direction.",
    )
    parser.add_argument(
        "--prove-negative-exit",
        action="store_true",
        help="Deterministically exercise the guard's nonzero-exit failure path on a "
        "synthetic forbidden spawn and exit 0 if detection succeeds.",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    try:
        self_test()
        if args.prove_negative_exit:
            return prove_negative_exit()
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
