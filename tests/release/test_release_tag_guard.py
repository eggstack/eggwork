"""Bounded tests for the pre-tag release guard.

The guard decides whether a release may be tagged, so its correctness is an
untested assumption unless something exercises it. These tests run the guard's
logic against fixtures rather than against the repository's own manifest, so a
reader of the guard can see what it accepts and what it refuses.

Two halves are covered:

- the **coherence** half, which needs no tag: the workspace manifest's version
  and the versions `Cargo.lock` pins for this repository's own packages agree.
  This is the half CI runs on every push and pull request.
- the **tag** half, which is the release gate: the tag names the manifest's
  version. No automated job runs this half, because no automated job knows the
  future tag; it is run by whoever cuts the tag.
"""

from __future__ import annotations

import contextlib
import io
import re
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "scripts"

sys.path.insert(0, str(SCRIPTS))
import check_release_tag as guard  # noqa: E402
import release_candidate_probe as probe  # noqa: E402

MANIFEST = """\
[workspace]
resolver = "2"
members = ["crates/eggwork-core"]

[workspace.package]
version = "0.1.5"
edition = "2024"
"""

#: A manifest whose ``[workspace.package]`` section is *not* first: an earlier
#: table carries a ``version =`` line of its own. A reader that takes the
#: first ``version =`` line it finds reads "9.9.9" here.
MANIFEST_PACKAGE_NOT_FIRST = """\
[workspace]
resolver = "2"
members = ["crates/eggwork-core"]

[workspace.metadata.release]
version = "9.9.9"

[workspace.package]
version = "0.1.5"
edition = "2024"
"""

LOCK = """\
version = 4

[[package]]
name = "eggwork-core"
version = "0.1.5"

[[package]]
name = "serde"
version = "1.0.219"
source = "registry+https://github.com/rust-lang/crates.io-index"
"""


def regex_workspace_version(text: str) -> str:
    """The reader this guard used before the corrective.

    Kept as a control: it demonstrates the failure mode the TOML reader
    removes, so the test that distinguishes them is not merely asserting
    that a number changed.
    """
    match = re.search(r'^version = "([^"]+)"', text, re.MULTILINE)
    return match.group(1) if match else ""


class FixtureRoot:
    """A throwaway repository root the guard can read."""

    def __init__(self, manifest: str = MANIFEST, lock: str = LOCK) -> None:
        self._temporary = tempfile.TemporaryDirectory()
        self.root = Path(self._temporary.name)
        (self.root / "Cargo.toml").write_text(manifest, encoding="utf-8")
        (self.root / "Cargo.lock").write_text(lock, encoding="utf-8")
        member = self.root / "crates" / "eggwork-core"
        member.mkdir(parents=True)
        (member / "Cargo.toml").write_text(
            '[package]\nname = "eggwork-core"\nversion.workspace = true\n',
            encoding="utf-8",
        )

    def close(self) -> None:
        self._temporary.cleanup()


class TagRuleTest(unittest.TestCase):
    """The tag↔version rule: the gate itself."""

    def setUp(self) -> None:
        self.fixture = FixtureRoot()
        self.addCleanup(self.fixture.close)

    def test_a_tag_with_the_v_prefix_matches(self) -> None:
        self.assertEqual(guard.tag_failures("v0.1.5", self.fixture.root), [])

    def test_a_tag_without_the_v_prefix_matches(self) -> None:
        self.assertEqual(guard.tag_failures("0.1.5", self.fixture.root), [])

    def test_a_mismatched_tag_is_refused(self) -> None:
        failures = guard.tag_failures("v0.1.6", self.fixture.root)
        self.assertTrue(failures, "a tag that names another version must fail")
        self.assertIn("0.1.5", failures[0])

    def test_the_v_prefix_is_the_only_permitted_difference(self) -> None:
        for tag in ("version-0.1.5", "v0.1.5-rc1", "0.1.5.1", "v"):
            with self.subTest(tag=tag):
                self.assertTrue(guard.tag_failures(tag, self.fixture.root))

    def test_the_guard_reads_the_manifest_through_the_shared_reader(self) -> None:
        calls: list[Path | None] = []
        original = probe.workspace_version

        def recording(root: Path | None = None) -> str:
            calls.append(root)
            return original(root)

        probe.workspace_version = recording  # type: ignore[assignment]
        self.addCleanup(lambda: setattr(probe, "workspace_version", original))
        self.assertEqual(guard.workspace_version(self.fixture.root), "0.1.5")
        self.assertEqual(calls, [self.fixture.root])

    def test_the_guard_does_not_parse_the_manifest_with_a_regular_expression(self) -> None:
        source = (SCRIPTS / "check_release_tag.py").read_text(encoding="utf-8")
        self.assertNotIn("\nimport re\n", source)
        self.assertNotIn("re.search", source)
        self.assertNotIn("re.compile", source)
        self.assertNotIn("re.MULTILINE", source)


class ManifestReaderTest(unittest.TestCase):
    """The reader: a real TOML reader scoped to ``[workspace.package]``."""

    def setUp(self) -> None:
        self.fixture = FixtureRoot()
        self.addCleanup(self.fixture.close)

    def test_the_reader_returns_the_package_version(self) -> None:
        self.assertEqual(probe.workspace_version(self.fixture.root), "0.1.5")

    def test_a_manifest_whose_package_section_is_not_first_still_reads_right(self) -> None:
        self.fixture = FixtureRoot(manifest=MANIFEST_PACKAGE_NOT_FIRST)
        self.addCleanup(self.fixture.close)
        self.assertEqual(probe.workspace_version(self.fixture.root), "0.1.5")
        # The control: the reader this replaced took the first `version =`
        # line in the file, which is not the one under [workspace.package].
        self.assertEqual(
            regex_workspace_version(MANIFEST_PACKAGE_NOT_FIRST), "9.9.9"
        )

    def test_a_malformed_manifest_fails_closed(self) -> None:
        self.fixture = FixtureRoot(manifest="not = [toml\n")
        self.addCleanup(self.fixture.close)
        with self.assertRaises(probe.ValidationFailure):
            probe.workspace_version(self.fixture.root)

    def test_a_manifest_without_a_package_version_fails_closed(self) -> None:
        self.fixture = FixtureRoot(manifest="[workspace]\nmembers = []\n")
        self.addCleanup(self.fixture.close)
        with self.assertRaises(probe.ValidationFailure):
            probe.workspace_version(self.fixture.root)

    def test_an_unreadable_manifest_fails_closed(self) -> None:
        self.fixture = FixtureRoot()
        self.addCleanup(self.fixture.close)
        (self.fixture.root / "Cargo.toml").unlink()
        with self.assertRaises(probe.ValidationFailure):
            probe.workspace_version(self.fixture.root)


class LockRuleTest(unittest.TestCase):
    """The lock half: the manifest and the lock agree."""

    def setUp(self) -> None:
        self.fixture = FixtureRoot()
        self.addCleanup(self.fixture.close)

    def test_a_coherent_lock_passes(self) -> None:
        self.assertEqual(guard.coherence_failures(self.fixture.root), [])

    def test_a_lock_pinned_at_another_version_fails(self) -> None:
        stale = LOCK.replace('name = "eggwork-core"\nversion = "0.1.5"',
                             'name = "eggwork-core"\nversion = "0.1.4"')
        self.fixture = FixtureRoot(lock=stale)
        self.addCleanup(self.fixture.close)
        failures = guard.coherence_failures(self.fixture.root)
        self.assertTrue(failures, "a stale lock pin must fail the coherence half")
        self.assertIn("eggwork-core", failures[0])

    def test_a_workspace_member_missing_from_the_lock_fails(self) -> None:
        without_member = """\
version = 4

[[package]]
name = "serde"
version = "1.0.219"
source = "registry+https://github.com/rust-lang/crates.io-index"
"""
        self.fixture = FixtureRoot(lock=without_member)
        self.addCleanup(self.fixture.close)
        failures = guard.coherence_failures(self.fixture.root)
        self.assertTrue(failures, "an unpinned workspace member must fail")
        self.assertIn("eggwork-core", failures[0])

    def test_registry_dependencies_are_not_ours_to_assert(self) -> None:
        locked = guard.lock_versions(self.fixture.root)
        self.assertEqual(locked, {"eggwork-core": "0.1.5"})

    def test_a_malformed_lock_fails_closed(self) -> None:
        self.fixture = FixtureRoot(lock="not = [toml\n")
        self.addCleanup(self.fixture.close)
        with self.assertRaises(guard.TagGuardFailure):
            guard.lock_versions(self.fixture.root)


class CommandLineTest(unittest.TestCase):
    """The command line a reader of the guard actually types."""

    def run_guard(self, argv: list[str]) -> int:
        """Run the guard's command line, keeping its output out of the report."""
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            return guard.main(argv)

    def test_the_repository_passes_the_coherence_half(self) -> None:
        self.assertEqual(self.run_guard(["--coherence"]), 0)

    def test_the_repository_passes_its_own_tag(self) -> None:
        version = probe.workspace_version()
        self.assertEqual(self.run_guard([f"v{version}"]), 0)
        self.assertEqual(self.run_guard([version]), 0)

    def test_a_wrong_tag_fails(self) -> None:
        self.assertEqual(self.run_guard(["v0.0.0-not-the-version"]), 1)

    def test_neither_a_tag_nor_the_coherence_flag_is_a_usage_error(self) -> None:
        with self.assertRaises(SystemExit):
            self.run_guard([])

    def test_a_tag_and_the_coherence_flag_together_is_a_usage_error(self) -> None:
        with self.assertRaises(SystemExit):
            self.run_guard(["--coherence", "v0.1.5"])


if __name__ == "__main__":
    unittest.main()
