"""Bounded, offline helpers shared by the Eggwork release candidate validators.

Eggpack invokes each validator as ``python3 <script> <candidate>`` from the
checked-out repository root. The single argument is the exact candidate path
Eggpack selected from the transferred build handoff for that target. Nothing
else is accepted: there is no release index, no network, no shell, and no
ambient Eggwork, Eggup, or service-manager consultation.

Candidate output is deliberately never returned. Eggpack records only the
validator's exit status, and these helpers print fixed reason phrases so a
failing run cannot leak candidate bytes into workflow logs or release
evidence.

Requires Python 3.11 or newer for :mod:`tomllib`.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

MAX_SOURCE_BYTES = 1 << 20
MAX_VERSION_BYTES = 64
MAX_CANDIDATE_STDOUT = 64 * 1024
MAX_CANDIDATE_STDERR = 64 * 1024
CANDIDATE_TIMEOUT_SECONDS = 30
_POSIX_ENV = ("PATH", "HOME", "TMPDIR", "LANG", "LC_ALL")
_WINDOWS_ENV = ("SYSTEMROOT", "SystemRoot", "COMSPEC", "PATHEXT", "WINDIR")


class ValidationFailure(Exception):
    """A bounded, name-only reason the candidate is not acceptable."""


def repository_root() -> Path:
    """Return the checked-out repository root containing this script."""
    return Path(__file__).resolve().parents[1]


def workspace_version() -> str:
    """Read ``[workspace.package].version`` from the checked-in manifest."""
    import tomllib

    manifest = repository_root() / "Cargo.toml"
    with manifest.open("rb") as handle:
        raw = handle.read(MAX_SOURCE_BYTES + 1)
    if len(raw) > MAX_SOURCE_BYTES:
        raise ValidationFailure("workspace manifest exceeds the size bound")
    try:
        document = tomllib.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        raise ValidationFailure("workspace manifest is not valid TOML") from error
    package = document.get("workspace", {}).get("package", {})
    version = package.get("version") if isinstance(package, dict) else None
    if not isinstance(version, str) or not 0 < len(version) <= MAX_VERSION_BYTES:
        raise ValidationFailure("workspace package version is missing or out of bounds")
    return version


def candidate_environment() -> dict[str, str]:
    """Build the minimal environment a candidate is allowed to observe."""
    names = _WINDOWS_ENV if os.name == "nt" else _POSIX_ENV
    return {name: os.environ[name] for name in names if name in os.environ}


def run_candidate(candidate: Path, argv: list[str]) -> bytes:
    """Run the exact candidate with a fixed argv and return bounded stdout."""
    if not candidate.is_file() or candidate.is_symlink():
        raise ValidationFailure("candidate is not a regular file")
    try:
        completed = subprocess.run(
            [str(candidate), *argv],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            shell=False,
            check=False,
            timeout=CANDIDATE_TIMEOUT_SECONDS,
            env=candidate_environment(),
        )
    except subprocess.TimeoutExpired as error:
        raise ValidationFailure("candidate exceeded its bounded runtime") from error
    except OSError as error:
        raise ValidationFailure("candidate could not be executed") from error
    if len(completed.stdout) > MAX_CANDIDATE_STDOUT:
        raise ValidationFailure("candidate stdout exceeds the bound")
    if len(completed.stderr) > MAX_CANDIDATE_STDERR:
        raise ValidationFailure("candidate stderr exceeds the bound")
    if completed.returncode != 0:
        raise ValidationFailure("candidate exited non-zero")
    return completed.stdout


def decode_bounded(raw: bytes) -> str:
    """Decode bounded candidate stdout, rejecting non-UTF-8 and control bytes."""
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ValidationFailure("candidate output is not valid UTF-8") from error
    if any(character < " " and character not in "\r\n\t" for character in text):
        raise ValidationFailure("candidate output contains control bytes")
    return text


def expected_version_line() -> str:
    """Return the single version line the candidate must report."""
    return workspace_version()


def run_entrypoint(check: "callable[[Path], None]") -> int:
    """Run ``check`` against the single candidate argument and map it to an exit code."""
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} CANDIDATE", file=sys.stderr)
        return 2
    try:
        check(Path(sys.argv[1]))
    except ValidationFailure as failure:
        print(f"validation failed: {failure}", file=sys.stderr)
        return 1
    print("validation passed")
    return 0
