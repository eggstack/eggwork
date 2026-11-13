"""Tests for the quickstart ↔ script agreement check.

Operations M008b §5.4 requires the quickstart to be machine-verified, and two
things make that true: the document's command blocks must match the
checked-in script, and the document's *shown output* must match what the node
reports. The plan records the concrete defect this catches — the document
shows seven ``doctor`` checks while ``crates/eggwork-server/src/operations.rs``
emits eight, including ``host-telemetry``.

``scripts/check_quickstart_sync.py`` is that check. These tests pin its
behaviour against synthetic fixtures, so they hold whatever state the live
document is in: the document's own conversion is a separate piece of work,
and a test that asserted the live document's agreement would either fail
today and pass tomorrow or pin the very drift the check exists to remove.
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
SYNC_CHECK = REPOSITORY_ROOT / "scripts" / "check_quickstart_sync.py"
QUICKSTART_SCRIPT = REPOSITORY_ROOT / "scripts" / "quickstart.sh"
QUICKSTART_DOCUMENT = REPOSITORY_ROOT / "docs" / "quickstart.md"

sys.path.insert(0, str(REPOSITORY_ROOT / "scripts"))

import check_quickstart_sync as sync  # noqa: E402

DOCUMENT = """\
# Quickstart

```bash
cargo build --locked -p eggwork-server --bin eggworkd
```

```bash
"$DAEMON" doctor --config "$ROOT/node.json"
```
"""

SCRIPT = """\
#!/usr/bin/env bash
set -euo pipefail

# step: build
# doc:begin
cargo build --locked -p eggwork-server --bin eggworkd
# doc:end

# step: doctor
# doc:begin
"$DAEMON" doctor --config "$ROOT/node.json"
# doc:end
"""


def compare(document: str, script: str) -> list[str]:
    """The disagreements between a synthetic document and a synthetic script."""
    failures, _script_map, _document_map = sync.compare_commands(
        sync.script_commands(script), sync.document_commands(document)
    )
    return failures


class CommandComparisonTests(unittest.TestCase):
    """The document ↔ script command comparison."""

    def test_a_matching_document_and_script_pass(self) -> None:
        """The same commands on both sides must produce no disagreement."""
        self.assertEqual(compare(DOCUMENT, SCRIPT), [])

    def test_a_command_the_script_runs_but_the_document_omits_fails(self) -> None:
        """A command only the script runs is drift, and must be reported."""
        script = SCRIPT + "\n# step: extra\n# doc:begin\ncargo test --locked -p demo\n# doc:end\n"
        failures = compare(DOCUMENT, script)
        self.assertEqual(len(failures), 1, failures)
        self.assertIn("scripts/quickstart.sh", failures[0])
        self.assertIn("cargo test --locked -p demo", failures[0])

    def test_a_command_the_document_shows_but_the_script_omits_fails(self) -> None:
        """A command only the document shows is drift, and must be reported."""
        document = DOCUMENT + "\n```bash\ncargo test --locked -p demo\n```\n"
        failures = compare(document, SCRIPT)
        self.assertEqual(len(failures), 1, failures)
        self.assertIn("docs/quickstart.md", failures[0])
        self.assertIn("cargo test --locked -p demo", failures[0])

    def test_paths_and_variables_are_compared_by_shape(self) -> None:
        """The document writes `/path/to/repo`; the script writes `$REPO_ROOT`."""
        document = (
            "```bash\n"
            "cd /path/to/repo\n"
            "./target/debug/eggworkd doctor --config \"$ROOT/node.json\"\n"
            "```\n"
        )
        script = (
            "# step: doctor\n"
            "# doc:begin\n"
            "cd \"$REPO_ROOT\"\n"
            "\"$DAEMON\" doctor --config \"$ROOT/node.json\"\n"
            "# doc:end\n"
        )
        self.assertEqual(compare(document, script), [])

    def test_a_literal_argument_difference_is_reported(self) -> None:
        """The shape keeps literal arguments, so a changed flag is drift."""
        document = "```bash\ncargo test -p demo\n```\n"
        script = "# step: t\n# doc:begin\ncargo test --locked -p demo\n# doc:end\n"
        failures = compare(document, script)
        self.assertEqual(len(failures), 2, failures)

    def test_a_heredoc_body_is_compared(self) -> None:
        """The configuration body is data the script writes; it must agree."""
        document = (
            "```bash\n"
            "cat > \"$ROOT/node.json\" <<EOF\n"
            "{\n"
            "  \"node_id\": \"worker-01\"\n"
            "}\n"
            "EOF\n"
            "```\n"
        )
        matching = (
            "# step: config\n"
            "# doc:begin\n"
            "cat > \"$ROOT/node.json\" <<EOF\n"
            "{\n"
            "  \"node_id\": \"worker-01\"\n"
            "}\n"
            "EOF\n"
            "# doc:end\n"
        )
        self.assertEqual(compare(document, matching), [])
        drifted = matching.replace("worker-01", "worker-02")
        failures = compare(document, drifted)
        self.assertEqual(len(failures), 2, failures)
        self.assertTrue(any("heredoc" in failure for failure in failures))

    def test_the_report_maps_document_blocks_to_script_steps(self) -> None:
        """The report is what tells the document's owner what to reconcile."""
        lines = sync.report_map(
            sync.script_commands(SCRIPT), sync.document_commands(DOCUMENT)
        )
        self.assertEqual(len(lines), 2, lines)
        self.assertTrue(any("script step build <-> document block 1" in line for line in lines))
        self.assertTrue(any("script step doctor <-> document block 2" in line for line in lines))


class DoctorOutputTests(unittest.TestCase):
    """The document's shown `doctor` output against the node's report."""

    DOCUMENT = """\
# Quickstart

```json
{
  "ready": true,
  "checks": [
    { "name": "configuration", "ok": true, "detail": "valid" },
    { "name": "listener", "ok": true, "detail": "available" }
  ]
}
```
"""

    REPORT = """\
{
  "ready": true,
  "checks": [
    { "name": "configuration", "ok": true, "detail": "valid" },
    { "name": "host-telemetry", "ok": true, "detail": "disabled" },
    { "name": "listener", "ok": true, "detail": "available" }
  ]
}
"""

    def check(self, report: str, document: str | None = None) -> list[str]:
        with tempfile.TemporaryDirectory(prefix="eggwork-quickstart-test-") as directory:
            path = Path(directory) / "doctor.json"
            path.write_text(report, encoding="utf-8")
            return sync.check_doctor_report(document or self.DOCUMENT, path)

    def test_matching_doctor_check_names_pass(self) -> None:
        """The same check names on both sides must pass."""
        report = self.REPORT.replace(
            '    { "name": "host-telemetry", "ok": true, "detail": "disabled" },\n', ""
        )
        self.assertEqual(self.check(report), [])

    def test_a_check_the_node_reports_but_the_document_omits_fails(self) -> None:
        """This is the seven-versus-eight mismatch the plan records."""
        failures = self.check(self.REPORT)
        self.assertEqual(len(failures), 1, failures)
        self.assertIn("host-telemetry", failures[0])

    def test_a_check_the_document_shows_but_the_node_omits_fails(self) -> None:
        """The comparison runs both ways, not only against the document."""
        report = self.REPORT.replace(
            '    { "name": "host-telemetry", "ok": true, "detail": "disabled" },\n', ""
        ).replace(
            '{ "name": "listener", "ok": true, "detail": "available" }',
            '{ "name": "listener", "ok": true, "detail": "available" },\n'
            '    { "name": "storage-headroom", "ok": true, "detail": "free" }',
        )
        failures = self.check(report)
        self.assertEqual(len(failures), 1, failures)
        self.assertIn("storage-headroom", failures[0])

    def test_a_document_without_a_doctor_block_fails_closed(self) -> None:
        """A document that shows no doctor output cannot be checked, and says so."""
        without_doctor = "# Quickstart\n\nNo output block here.\n"
        failures = self.check(self.REPORT, document=without_doctor)
        self.assertEqual(len(failures), 1, failures)
        self.assertIn("shows no doctor output block", failures[0])


class TrustedHelperPathTests(unittest.TestCase):
    """The helper's trust rule, exercised as the script executes it.

    The helper is trusted only when it is not group- or world-writable and
    every ancestor directory is non-writable. A directory that fails the rule
    makes the node refuse the isolation it is required to enforce, and that
    refusal reads like a product defect unless the script says so first.

    These tests run the script's own shell functions, because the rule lives
    in the script and a Python re-implementation would be a second owner of
    it. An inverted condition here (the directory *is* trusted but the check
    rejects it) is exactly the defect an end-to-end run finds and a unit
    test of a re-implementation would not.
    """

    FUNCTIONS = ("is_not_writable_by_others", "check_trusted_path")

    def functions(self) -> str:
        """The trust-checking functions, extracted from the checked-in script."""
        script = QUICKSTART_SCRIPT.read_text(encoding="utf-8")
        lines = script.split("\n")
        extracted: list[str] = []
        for name in self.FUNCTIONS:
            start = next(
                index for index, line in enumerate(lines) if line.startswith(f"{name}() {{")
            )
            for line in lines[start:]:
                extracted.append(line)
                if line == "}":
                    break
        return "\n".join(extracted)

    def run_bash(self, program: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["bash", "-c", program], capture_output=True, text=True, check=False
        )

    def test_a_private_directory_is_trusted(self) -> None:
        """A directory nobody but the owner may write is trusted."""
        with tempfile.TemporaryDirectory(prefix="eggwork-trust-test-") as directory:
            path = Path(directory)
            (path / "private").mkdir()
            (path / "private").chmod(0o755)
            (path / "group").mkdir()
            (path / "group").chmod(0o775)
            (path / "world").mkdir()
            (path / "world").chmod(0o777)
            program = (
                "set -euo pipefail\n"
                + self.functions()
                + f'\nis_not_writable_by_others "{path}/private"\n'
                + "echo TRUSTED\n"
            )
            result = self.run_bash(program)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("TRUSTED", result.stdout)

    def test_a_group_writable_directory_is_not_trusted(self) -> None:
        """A group-writable directory makes the helper untrusted."""
        with tempfile.TemporaryDirectory(prefix="eggwork-trust-test-") as directory:
            path = Path(directory) / "group"
            path.mkdir()
            path.chmod(0o775)
            program = (
                "set -euo pipefail\n"
                + self.functions()
                + f'\nis_not_writable_by_others "{path}"\n'
            )
            result = self.run_bash(program)
            self.assertNotEqual(
                result.returncode,
                0,
                "a group-writable directory must not pass the trust check",
            )

    def test_a_world_writable_directory_is_not_trusted(self) -> None:
        """A world-writable directory makes the helper untrusted."""
        with tempfile.TemporaryDirectory(prefix="eggwork-trust-test-") as directory:
            path = Path(directory) / "world"
            path.mkdir()
            path.chmod(0o777)
            program = (
                "set -euo pipefail\n"
                + self.functions()
                + f'\nis_not_writable_by_others "{path}"\n'
            )
            result = self.run_bash(program)
            self.assertNotEqual(
                result.returncode,
                0,
                "a world-writable directory must not pass the trust check",
            )

    def test_the_trust_check_names_the_offending_directory(self) -> None:
        """A failure must name the directory that made the helper untrusted."""
        with tempfile.TemporaryDirectory(prefix="eggwork-trust-test-") as directory:
            path = Path(directory)
            (path / "parent").mkdir()
            (path / "parent").chmod(0o755)
            (path / "parent" / "untrusted").mkdir()
            (path / "parent" / "untrusted").chmod(0o777)
            program = (
                'set -euo pipefail\n'
                'CURRENT_STEP="3"\n'
                'CURRENT_STEP_TITLE="Install the helper where it is trusted"\n'
                + self.functions()
                + f'\ncheck_trusted_path "{path}/parent/untrusted/eggwork-sandbox-helper"\n'
            )
            result = self.run_bash(program)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("group- or world-writable directory", result.stderr)
            self.assertIn("untrusted", result.stderr)


class CheckedInArtifactTests(unittest.TestCase):
    """Properties the checked-in script and document must have."""

    def test_the_checked_in_script_uses_strict_mode(self) -> None:
        """`set -euo pipefail` is what makes a failing step fail the job."""
        script = QUICKSTART_SCRIPT.read_text(encoding="utf-8")
        self.assertIn("set -euo pipefail", script)

    def test_the_checked_in_script_is_executable(self) -> None:
        """The script is run, not sourced."""
        self.assertTrue(
            QUICKSTART_SCRIPT.stat().st_mode & 0o111,
            "scripts/quickstart.sh must be executable",
        )

    def test_the_checked_in_script_declares_a_documented_region_per_step(self) -> None:
        """Every step must mark the commands the document has to show."""
        script = QUICKSTART_SCRIPT.read_text(encoding="utf-8")
        commands = sync.script_commands(script)
        steps = {command.label for command in commands}
        self.assertGreaterEqual(len(steps), 7, steps)
        self.assertNotIn("(unmarked)", steps)

    def test_the_checked_in_script_has_no_undocumented_step(self) -> None:
        """A step with no `# doc:begin` region would be invisible to the check."""
        script = QUICKSTART_SCRIPT.read_text(encoding="utf-8")
        for marker in ("# step:", "# doc:begin", "# doc:end"):
            self.assertIn(marker, script)

    def test_the_checked_in_script_and_document_can_be_compared(self) -> None:
        """The comparison must run over the live pair without erroring."""
        result = subprocess.run(
            [sys.executable, str(SYNC_CHECK), "--report"],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("script step", result.stdout)

    def test_the_checked_in_script_names_the_failing_step(self) -> None:
        """A red job must be diagnosable from the log alone."""
        script = QUICKSTART_SCRIPT.read_text(encoding="utf-8")
        self.assertIn("quickstart FAILED at step", script)


if __name__ == "__main__":
    unittest.main()
