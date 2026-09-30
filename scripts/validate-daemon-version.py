#!/usr/bin/env python3
"""Gating release evidence for the Eggwork daemon candidate.

Eggpack selects this candidate as the direct artifact of a macOS or Windows
release target, or as bundle entry 0 of a Linux target, and runs
``eggworkd version`` on it after core qualification. The probe is bounded,
offline, shell-free, and consults no ambient Eggwork, Eggup, or node
configuration.

The daemon reports its package version as a JSON object, so the check requires
that object and requires its ``version`` to equal
``[workspace.package].version``. This turns "the candidate exits zero" into a
version-coherence claim on every required target, not only on Linux.
"""

from __future__ import annotations

import json
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
    """Require bounded ``eggworkd version`` JSON naming the workspace version."""
    stdout = run_candidate(candidate, ["version"])
    try:
        report = json.loads(decode_bounded(stdout))
    except json.JSONDecodeError as error:
        raise ValidationFailure("daemon did not report version JSON") from error
    if not isinstance(report, dict):
        raise ValidationFailure("daemon version output is not a JSON object")
    reported = report.get("version")
    if not isinstance(reported, str) or not reported:
        raise ValidationFailure("daemon version output has no version field")
    if reported != expected_version_line():
        raise ValidationFailure("daemon version differs from the workspace package version")


if __name__ == "__main__":
    raise SystemExit(run_entrypoint(check))
