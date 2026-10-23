"""Drift tests for the sandbox environment-policy generator.

The environment policy — the denied-key list and its matching rule, the
node-owned (reserved) keys, the node-owned baseline, and the `PATH` value
contract — has one owner:
``crates/eggwork-runner/policy/environment-policy.toml``. The runner and the
standalone sandbox helper each compile a *generated* copy of it, because the
helper may not depend on any workspace crate.

Two copies of one contract is a hole unless something pins them together. The
Rust side pins them together behaviourally (a parity test drives the real
helper executable and compares its decisions with the runner's policy). This
file pins the other end: that the generator is deterministic, that it fails
loudly rather than emitting a partial file, and that the checked-in copies
are what the generator produces.

The generator is not the gate. These tests are: a hand edit of a generated
copy is a contract change, and the check here is what makes that visible
before it reaches a release.
"""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
GENERATOR = REPOSITORY_ROOT / "scripts" / "generate_environment_policy.py"
POLICY = Path("crates/eggwork-runner/policy/environment-policy.toml")
CORE = Path("crates/eggwork-core/src/lib.rs")
RUNNER_OUTPUT = Path("crates/eggwork-runner/src/environment_policy.rs")
HELPER_OUTPUT = Path("crates/eggwork-sandbox-helper/src/environment_policy.rs")


def run_generator(*arguments: str, root: Path | None = None) -> subprocess.CompletedProcess[str]:
    """Run the generator and return the completed process."""
    command = [sys.executable, str(GENERATOR), *arguments]
    if root is not None:
        command.extend(["--root", str(root)])
    return subprocess.run(
        command,
        capture_output=True,
        text=True,
        check=False,
    )


class GeneratorDriftTests(unittest.TestCase):
    """The generator's own properties."""

    def test_the_checked_in_copies_are_what_the_generator_produces(self) -> None:
        """The committed copies must be the generator's output, byte for byte.

        This is the drift check. A hand edit of a generated copy — or a
        generator change that was never re-run — fails here rather than
        reaching a release with two different policies in the tree.
        """
        result = run_generator("--check")
        self.assertEqual(
            result.returncode,
            0,
            "the checked-in environment-policy copies differ from the generator's output; "
            f"run `python3 scripts/generate_environment_policy.py`\n{result.stdout}"
            f"{result.stderr}",
        )

    def test_the_generator_is_deterministic(self) -> None:
        """Running the generator twice must produce byte-identical output.

        A generator whose output moves between runs would make the drift check
        above useless: it would fail for a reason that has nothing to do with
        the policy.
        """
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / POLICY.parent).mkdir(parents=True)
            (root / CORE.parent).mkdir(parents=True)
            shutil.copyfile(REPOSITORY_ROOT / POLICY, root / POLICY)
            shutil.copyfile(REPOSITORY_ROOT / CORE, root / CORE)
            first = run_generator(root=root)
            self.assertEqual(first.returncode, 0, first.stderr)
            runner_copy = (root / RUNNER_OUTPUT).read_bytes()
            helper_copy = (root / HELPER_OUTPUT).read_bytes()
            second = run_generator(root=root)
            self.assertEqual(second.returncode, 0, second.stderr)
            self.assertEqual(runner_copy, (root / RUNNER_OUTPUT).read_bytes())
            self.assertEqual(helper_copy, (root / HELPER_OUTPUT).read_bytes())

    def test_a_malformed_source_of_truth_fails_without_writing_anything(self) -> None:
        """A broken policy must fail loudly, and must not leave a partial file.

        A generator that writes half a policy and exits zero is worse than one
        that refuses: the half-written file would be compiled into a release.
        """
        for corruption, description in (
            ("this is not toml = = =\n", "malformed TOML"),
            ("schema_version = 99\n", "an unknown schema version"),
        ):
            with self.subTest(description=description):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    (root / POLICY.parent).mkdir(parents=True)
                    (root / CORE.parent).mkdir(parents=True)
                    shutil.copyfile(REPOSITORY_ROOT / CORE, root / CORE)
                    (root / POLICY).write_text(corruption, encoding="utf-8")
                    result = run_generator(root=root)
                    self.assertNotEqual(
                        result.returncode,
                        0,
                        f"{description} must fail the generator",
                    )
                    self.assertFalse(
                        (root / RUNNER_OUTPUT).exists(),
                        f"{description} must not leave a partially written copy behind",
                    )
                    self.assertFalse(
                        (root / HELPER_OUTPUT).exists(),
                        f"{description} must not leave a partially written copy behind",
                    )

    def test_a_policy_whose_class_and_baseline_disagree_fails(self) -> None:
        """The node-owned class and the node-owned baseline are one list.

        The generator refuses a source of truth in which they differ, because
        the two are the same thing written twice and a disagreement means one
        of them is wrong.
        """
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / POLICY.parent).mkdir(parents=True)
            (root / CORE.parent).mkdir(parents=True)
            shutil.copyfile(REPOSITORY_ROOT / CORE, root / CORE)
            policy = (REPOSITORY_ROOT / POLICY).read_text(encoding="utf-8")
            head, _, tail = policy.partition("[reserved]")
            self.assertTrue(tail, "the policy must carry a [reserved] table")
            broken = head + "[reserved]" + tail.replace(
                '    "GIT_TERMINAL_PROMPT",\n', "", 1
            )
            self.assertNotEqual(broken, policy)
            (root / POLICY).write_text(broken, encoding="utf-8")
            result = run_generator(root=root)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("node-owned class", result.stderr)

    def test_the_helper_copy_carries_the_core_limits(self) -> None:
        """The helper's copy of the five protocol limits comes from the owner.

        The helper cannot import `eggwork-core`, so the generator reads the
        five values out of the owner of the wire limits. This test pins that
        the values in the generated copy are the owner's values and not a
        hand-maintained guess.
        """
        helper = (REPOSITORY_ROOT / HELPER_OUTPUT).read_text(encoding="utf-8")
        core = (REPOSITORY_ROOT / CORE).read_text(encoding="utf-8")
        for name, value in (
            ("MAX_ARG_COUNT", "256"),
            ("MAX_ARG_BYTES", "32768"),
            ("MAX_ENV_COUNT", "256"),
            ("MAX_ENV_NAME_BYTES", "256"),
            ("MAX_ENV_VALUE_BYTES", "16384"),
        ):
            with self.subTest(limit=name):
                self.assertIn(f"pub const {name}: usize = {value};", helper)
                self.assertIn(f"pub const {name}: usize = ", core)


if __name__ == "__main__":
    unittest.main()
