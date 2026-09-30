#!/usr/bin/env python3
"""Gating release evidence for the Eggwork Linux sandbox helper candidate.

Eggpack selects this candidate as bundle entry 1 of a Linux release target and
runs ``eggwork-sandbox-helper --version`` on it after core qualification of the
daemon. The probe is bounded, offline, shell-free, and consults no ambient
Eggwork or Eggup service. It complements core qualification of the daemon; it
is not a second qualification implementation.

The accepted output is exactly one bounded non-empty version line that equals
``[workspace.package].version``. That is the same identity
``deployment::check_helper_compatibility`` requires at update time, so a helper
whose version drifts from the daemon is refused before the release can ship.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from release_candidate_probe import (  # noqa: E402
    ValidationFailure,
    decode_bounded,
    expected_version_line,
    run_candidate,
    run_entrypoint,
)


def check(candidate: Path) -> None:
    """Require one bounded version line equal to the workspace package version."""
    stdout = run_candidate(candidate, ["--version"])
    lines = decode_bounded(stdout).splitlines()
    if len(lines) != 1:
        raise ValidationFailure("helper did not report exactly one version line")
    reported = lines[0].strip()
    if not reported:
        raise ValidationFailure("helper reported an empty version line")
    if reported != expected_version_line():
        raise ValidationFailure("helper version differs from the workspace package version")


if __name__ == "__main__":
    raise SystemExit(run_entrypoint(check))
