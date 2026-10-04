#!/usr/bin/env python3
"""Pre-tag guard for Eggwork releases: the tag, the version, and the lock agree.

Two staged releases failed for reasons this script would have caught locally:

- `v0.1.3` never staged because the version bump changed `Cargo.toml` without
  regenerating `Cargo.lock`, so every `--locked` release build failed closed.
- `v0.1.4` staged binaries reporting `0.1.3` because the version bump was
  missed entirely, and hosted qualification correctly refused them with
  `installed-daemon-version: daemon reports '0.1.3', expected 0.1.4`.

The tag-to-version link cannot live in CI: the generated release workflow is
producer authority (the `release-drift` job rejects hand edits to it), and CI
never knows the future tag. So the check lives here, run by whoever cuts the
tag, before the tag exists:

    python3 scripts/check_release_tag.py v0.1.5
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def workspace_version() -> str:
    text = (ROOT / "Cargo.toml").read_text()
    match = re.search(r'^version = "([^"]+)"', text, re.MULTILINE)
    if not match:
        return ""
    return match.group(1)


def lock_versions() -> dict[str, str]:
    text = (ROOT / "Cargo.lock").read_text()
    found: dict[str, str] = {}
    for block in text.split("[[package]]"):
        name = re.search(r'^name = "([^"]+)"', block, re.MULTILINE)
        version = re.search(r'^version = "([^"]+)"', block, re.MULTILINE)
        if name and version and name.group(1).startswith("eggwork-"):
            found[name.group(1)] = version.group(1)
    return found


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tag", help="exact tag about to be created, e.g. v0.1.5")
    args = parser.parse_args()

    expected = args.tag[1:] if args.tag.startswith("v") else args.tag
    version = workspace_version()
    failures: list[str] = []
    if version != expected:
        failures.append(
            f"workspace version is {version!r}, tag {args.tag!r} expects {expected!r}"
        )
    for name, locked in sorted(lock_versions().items()):
        if locked != expected:
            failures.append(f"Cargo.lock pins {name} at {locked!r}, expected {expected!r}")
    if failures:
        print("release tag guard FAILED:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        print("bump Cargo.toml, regenerate the lock, rerun the gates, then tag.",
              file=sys.stderr)
        return 1
    print(f"tag {args.tag!r} matches workspace version and lock ({expected})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
