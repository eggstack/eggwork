"""Bounded tests for the Eggwork release candidate validators.

These cover the negative cases the release gate depends on: a wrong version, a
wrong output shape, a wrong candidate identity, an unusable candidate, and any
leak of candidate output back into the validator's own result.
"""

from __future__ import annotations

import importlib.util
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "scripts"
PROBE_PATH = SCRIPTS / "release_candidate_probe.py"
HELPER_VALIDATOR = SCRIPTS / "validate-sandbox-helper.py"
DAEMON_VALIDATOR = SCRIPTS / "validate-daemon-version.py"

sys.path.insert(0, str(SCRIPTS))
import release_candidate_probe as probe  # noqa: E402


def load(path: Path):
    spec = importlib.util.spec_from_file_location(path.stem.replace("-", "_"), path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


helper = load(HELPER_VALIDATOR)
daemon = load(DAEMON_VALIDATOR)


def candidate(tmp: Path, name: str, body: str) -> Path:
    """Write an executable fake candidate and return its path."""
    path = tmp / name
    path.write_text(f'#!{sys.executable}\n{body}', encoding="utf-8")
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return path


HELPER_OK = "import sys\nprint('0.1.0')\n"
HELPER_WRONG_VERSION = "print('9.9.9')\n"
HELPER_EMPTY = "print('')\n"
HELPER_TWO_LINES = "print('0.1.0')\nprint('0.1.0')\n"
HELPER_DAEMON_SHAPED = "import json;print(json.dumps({'version':'0.1.0'}))\n"
DAEMON_OK = "import json;print(json.dumps({'version':'0.1.0'}))\n"
DAEMON_WRONG_VERSION = "import json;print(json.dumps({'version':'9.9.9'}))\n"
DAEMON_NO_FIELD = "import json;print(json.dumps({'schema_version':1}))\n"
DAEMON_HELPER_SHAPED = "print('0.1.0')\n"


class ValidatorTest(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)

    def assertFails(self, check, path: Path) -> str:
        with self.assertRaises(probe.ValidationFailure) as caught:
            check(path)
        return str(caught.exception)

    def test_helper_accepts_the_workspace_version(self) -> None:
        helper.check(candidate(self.tmp, "helper-ok", HELPER_OK))

    def test_helper_rejects_a_different_version(self) -> None:
        reason = self.assertFails(
            helper.check, candidate(self.tmp, "helper-skew", HELPER_WRONG_VERSION)
        )
        self.assertIn("differs from the workspace package version", reason)

    def test_helper_rejects_an_empty_version_line(self) -> None:
        reason = self.assertFails(
            helper.check, candidate(self.tmp, "helper-empty", HELPER_EMPTY)
        )
        self.assertIn("empty version line", reason)

    def test_helper_rejects_more_than_one_line(self) -> None:
        reason = self.assertFails(
            helper.check, candidate(self.tmp, "helper-two", HELPER_TWO_LINES)
        )
        self.assertIn("exactly one version line", reason)

    def test_helper_rejects_the_daemon_as_a_substitute(self) -> None:
        reason = self.assertFails(
            helper.check, candidate(self.tmp, "not-a-helper", HELPER_DAEMON_SHAPED)
        )
        self.assertIn("differs from the workspace package version", reason)

    def test_helper_rejects_a_nonzero_exit(self) -> None:
        reason = self.assertFails(
            helper.check,
            candidate(self.tmp, "helper-fail", "import sys\nsys.exit(3)\n"),
        )
        self.assertIn("exited non-zero", reason)

    def test_helper_rejects_non_utf8_output(self) -> None:
        reason = self.assertFails(
            helper.check,
            candidate(self.tmp, "helper-binary", "import sys\nsys.stdout.buffer.write(b'\\xff')\n"),
        )
        self.assertIn("not valid UTF-8", reason)

    def test_daemon_accepts_the_workspace_version(self) -> None:
        daemon.check(candidate(self.tmp, "daemon-ok", DAEMON_OK))

    def test_daemon_rejects_a_different_version(self) -> None:
        reason = self.assertFails(
            daemon.check, candidate(self.tmp, "daemon-skew", DAEMON_WRONG_VERSION)
        )
        self.assertIn("differs from the workspace package version", reason)

    def test_daemon_rejects_a_missing_version_field(self) -> None:
        reason = self.assertFails(
            daemon.check, candidate(self.tmp, "daemon-empty", DAEMON_NO_FIELD)
        )
        self.assertIn("no version field", reason)

    def test_daemon_rejects_the_helper_as_a_substitute(self) -> None:
        reason = self.assertFails(
            daemon.check, candidate(self.tmp, "not-a-daemon", DAEMON_HELPER_SHAPED)
        )
        self.assertIn("version JSON", reason)

    def test_daemon_rejects_output_that_is_not_json(self) -> None:
        reason = self.assertFails(
            daemon.check, candidate(self.tmp, "daemon-plain", "print('hello')\n")
        )
        self.assertIn("version JSON", reason)

    def test_validators_reject_an_unusable_candidate(self) -> None:
        for check in (helper.check, daemon.check):
            with self.assertRaises(probe.ValidationFailure):
                check(self.tmp / "absent")
        directory = self.tmp / "directory"
        directory.mkdir()
        with self.assertRaises(probe.ValidationFailure):
            helper.check(directory)

    def test_validators_reject_a_symlinked_candidate(self) -> None:
        real = candidate(self.tmp, "real-helper", HELPER_OK)
        link = self.tmp / "link-helper"
        link.symlink_to(real)
        with self.assertRaises(probe.ValidationFailure):
            helper.check(link)


class EntrypointTest(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)

    def invoke(self, script: Path, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(script), *args],
            capture_output=True,
            text=True,
            check=False,
            cwd=str(ROOT),
        )

    def test_entrypoint_accepts_exactly_one_argument(self) -> None:
        self.assertEqual(self.invoke(HELPER_VALIDATOR).returncode, 2)
        self.assertEqual(
            self.invoke(HELPER_VALIDATOR, "a", "b").returncode,
            2,
        )

    def test_entrypoint_reports_a_fixed_reason_without_candidate_output(self) -> None:
        secret = "candidate-secret-9f2c"
        noisy = candidate(
            self.tmp,
            "noisy-helper",
            f"import sys\nprint({secret!r})\nsys.exit(4)\n",
        )
        result = self.invoke(HELPER_VALIDATOR, str(noisy))
        self.assertEqual(result.returncode, 1)
        self.assertNotIn(secret, result.stdout)
        self.assertNotIn(secret, result.stderr)
        self.assertIn("exited non-zero", result.stderr)


class WorkspaceVersionTest(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)
        original = probe.repository_root
        probe.repository_root = lambda: self.tmp  # type: ignore[assignment]
        self.addCleanup(lambda: setattr(probe, "repository_root", original))

    def test_workspace_version_fails_closed_on_a_malformed_manifest(self) -> None:
        (self.tmp / "Cargo.toml").write_text("not = [toml\n", encoding="utf-8")
        with self.assertRaises(probe.ValidationFailure):
            probe.workspace_version()

    def test_workspace_version_fails_closed_without_a_package_version(self) -> None:
        (self.tmp / "Cargo.toml").write_text(
            "[workspace]\nmembers = []\n", encoding="utf-8"
        )
        with self.assertRaises(probe.ValidationFailure):
            probe.workspace_version()

    def test_workspace_version_rejects_an_out_of_bounds_value(self) -> None:
        (self.tmp / "Cargo.toml").write_text(
            '[workspace.package]\nversion = ""\n', encoding="utf-8"
        )
        with self.assertRaises(probe.ValidationFailure):
            probe.workspace_version()


class BuiltCandidateTest(unittest.TestCase):
    """Positive evidence against the exact binaries this repository builds.

    Skipped when the workspace has not been built, so the unit suite never
    forces a Cargo build. The hosted five-target release workflow is the
    authoritative cross-platform evidence.
    """

    def binary(self, name: str) -> Path:
        for profile in ("debug", "release"):
            candidate = ROOT / "target" / profile / name
            if candidate.is_file():
                return candidate
        self.skipTest(f"{name} is not built; run cargo build first")

    def test_built_helper_reports_the_workspace_version(self) -> None:
        helper.check(self.binary("eggwork-sandbox-helper"))

    def test_built_daemon_reports_the_workspace_version(self) -> None:
        daemon.check(self.binary("eggworkd"))


class BoundednessTest(unittest.TestCase):
    def source(self, path: Path) -> str:
        return path.read_text(encoding="utf-8")

    def test_workspace_version_matches_the_checked_in_manifest(self) -> None:
        manifest = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
        self.assertIn(f'version = "{probe.workspace_version()}"', manifest)

    def test_probe_never_shells_out(self) -> None:
        self.assertIn("shell=False", self.source(PROBE_PATH))
        for forbidden in ("os.system", "os.popen", "subprocess.getoutput", "shell=True"):
            self.assertNotIn(forbidden, self.source(PROBE_PATH))

    def test_probe_never_reaches_the_network(self) -> None:
        text = self.source(PROBE_PATH)
        for forbidden in ("socket", "urllib", "http.client", "requests", "curl"):
            self.assertNotIn(forbidden, text)

    def test_candidate_environment_is_an_allowlist(self) -> None:
        environment = probe.candidate_environment()
        self.assertTrue(set(environment) <= set(probe._POSIX_ENV + probe._WINDOWS_ENV))
        self.assertNotIn("GITHUB_TOKEN", environment)

    def test_validators_take_only_the_candidate_path(self) -> None:
        for script in (HELPER_VALIDATOR, DAEMON_VALIDATOR):
            with self.assertRaises(subprocess.CalledProcessError):
                subprocess.run(
                    [sys.executable, str(script), "--release-tag", "v9.9.9"],
                    capture_output=True,
                    check=True,
                    cwd=str(ROOT),
                )


if __name__ == "__main__":
    unittest.main()
