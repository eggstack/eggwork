#!/usr/bin/env python3
"""Print the per-crate source size of this workspace, on demand.

The design record used to carry a checked-in line count per crate. A checked-in
count is a fact about the tree that rots on the next edit and is wrong the
moment it is written, so the record now points at this script instead of
carrying the number. Run it to get the current figures::

    python3 scripts/component_sizes.py            # a table
    python3 scripts/component_sizes.py --markdown # the same, as a table row set
    python3 scripts/component_sizes.py --json     # machine-readable

The measure is **source lines**: every line of every ``.rs`` file a crate
owns, split into the crate's ``src/`` (the product) and its ``tests/`` (the
evidence). Blank lines and comment-only lines are counted, because the figure
is a size, not a density. The counts are per crate and sum to the workspace
total, so a reader can check the arithmetic.

The script is stdlib-only and deterministic: the same tree always produces the
same output, and the output depends on nothing but the files on disk.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CRATES = ROOT / "crates"

# The crates in the order the design record lists them.
CRATE_ORDER = [
    "eggwork-core",
    "eggwork-runner",
    "eggwork-sandbox-helper",
    "eggwork-client",
    "eggwork-server",
]

SOURCE_DIRECTORIES = ("src", "tests")


def count_lines(path: Path) -> int:
    """The number of lines in a file, counted the way `wc -l` counts them."""
    with path.open("rb") as handle:
        return sum(1 for _ in handle)


def crate_files(crate: Path, directory: str) -> list[Path]:
    """Every Rust source file a crate owns in one directory, in a stable order."""
    base = crate / directory
    if not base.is_dir():
        return []
    return sorted(base.rglob("*.rs"))


def measure() -> dict[str, dict[str, int]]:
    """The per-crate line counts, keyed by crate name."""
    measured: dict[str, dict[str, int]] = {}
    for crate in sorted(CRATES.iterdir()) if CRATES.is_dir() else []:
        if not crate.is_dir():
            continue
        counts: dict[str, int] = {}
        for directory in SOURCE_DIRECTORIES:
            files = crate_files(crate, directory)
            counts[directory] = sum(count_lines(path) for path in files)
            counts[f"{directory}_files"] = len(files)
        counts["lines"] = counts["src"] + counts["tests"]
        counts["files"] = counts["src_files"] + counts["tests_files"]
        measured[crate.name] = counts
    return measured


def ordered(measured: dict[str, dict[str, int]]) -> list[tuple[str, dict[str, int]]]:
    """The measured crates, in the design record's order, then any new ones."""
    ordered_names = [name for name in CRATE_ORDER if name in measured]
    ordered_names += [name for name in sorted(measured) if name not in CRATE_ORDER]
    return [(name, measured[name]) for name in ordered_names]


def render_table(rows: list[tuple[str, dict[str, int]]]) -> str:
    lines = [
        "| Crate | `src/` lines | `tests/` lines | files | total lines |",
        "|---|---|---|---|---|",
    ]
    for name, counts in rows:
        lines.append(
            f"| `{name}` | {counts['src']:,} | {counts['tests']:,} | "
            f"{counts['files']} | {counts['lines']:,} |"
        )
    totals = {
        key: sum(counts[key] for _, counts in rows)
        for key in ("src", "tests", "files", "lines")
    }
    lines.append(
        f"| **total** | **{totals['src']:,}** | **{totals['tests']:,}** | "
        f"**{totals['files']}** | **{totals['lines']:,}** |"
    )
    return "\n".join(lines)


def render_markdown(rows: list[tuple[str, dict[str, int]]]) -> str:
    """One table row per crate, in the shape the design record's table used."""
    lines = []
    for name, counts in rows:
        lines.append(
            f"| `{name}` | {counts['src']:,} LOC in `src/`, "
            f"{counts['tests']:,} LOC in `tests/` |"
        )
    return "\n".join(lines)


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Print the per-crate source size of this workspace."
    )
    parser.add_argument(
        "--markdown",
        action="store_true",
        help="print one table row per crate instead of the full table",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="print the counts as JSON",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    measured = measure()
    if not measured:
        print(
            "component size: no crates found under crates/ — the measurement has "
            "nothing to report",
            file=sys.stderr,
        )
        return 1
    rows = ordered(measured)
    if args.json:
        print(json.dumps({name: counts for name, counts in rows}, indent=2))
        return 0
    if args.markdown:
        print(render_markdown(rows))
        return 0
    print(render_table(rows))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
