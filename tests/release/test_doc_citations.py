"""Tests for the documentation citation guard.

``scripts/check_doc_citations.py`` is the durable gate that keeps the design
record citing *symbols* rather than line numbers (Operations M008b §5.3). Its
value is entirely in its failure path, so these tests exercise that path
against synthetic fixtures: a correct citation must pass, and every class of
wrong citation must be rejected.

The fixtures are synthetic on purpose. The live design record still carries
its legacy line-number citations — the conversion is a separate piece of work
— so a test that ran against the live tree would either fail today and pass
tomorrow, or assert the very drift the gate exists to remove. These tests
therefore pin the *rule set*, which is what has to survive the conversion.
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
CHECKER = REPOSITORY_ROOT / "scripts" / "check_doc_citations.py"

sys.path.insert(0, str(REPOSITORY_ROOT / "scripts"))

import check_doc_citations as checker  # noqa: E402

RUST_SOURCE = """\
// A fixture source file.
pub const REAL_CONSTANT: usize = 7;
static REAL_STATIC: usize = 9;
pub struct RealStruct {
    pub field: usize,
}
pub enum RealEnum {
    First,
    Second,
}
pub trait RealTrait {
    fn trait_method(&self);
}
pub type RealAlias = usize;
pub mod real_module {
    pub fn module_function() {}
}
macro_rules! real_macro {
    () => {};
}
pub fn real_function() {}
impl RealStruct {
    pub fn real_method(&self) -> usize {
        self.field
    }
}
"""

PYTHON_SOURCE = """\
REAL_CONSTANT = 3


def real_function():
    return REAL_CONSTANT


class RealClass:
    def real_method(self):
        return REAL_CONSTANT
"""


def build_tree(root: Path, documents: dict[str, str]) -> None:
    """Materialise a synthetic repository the checker can run against."""
    (root / "architecture").mkdir(parents=True, exist_ok=True)
    (root / "docs").mkdir(parents=True, exist_ok=True)
    (root / "crates" / "demo" / "src").mkdir(parents=True, exist_ok=True)
    (root / "crates" / "other" / "src").mkdir(parents=True, exist_ok=True)
    (root / "scripts").mkdir(parents=True, exist_ok=True)
    (root / "crates" / "demo" / "src" / "lib.rs").write_text(
        RUST_SOURCE, encoding="utf-8"
    )
    # A second `lib.rs`, so that a bare `lib.rs` citation is ambiguous.
    (root / "crates" / "other" / "src" / "lib.rs").write_text(
        "pub fn unrelated() {}\n", encoding="utf-8"
    )
    (root / "crates" / "demo" / "src" / "store.rs").write_text(
        "pub fn store_function() {}\n", encoding="utf-8"
    )
    (root / "scripts" / "demo.py").write_text(PYTHON_SOURCE, encoding="utf-8")
    for name, body in documents.items():
        (root / name).write_text(body, encoding="utf-8")


def findings_for(documents: dict[str, str]) -> list[checker.Finding]:
    """Run the production check over a synthetic tree and return the findings."""
    with tempfile.TemporaryDirectory(prefix="eggwork-citation-test-") as directory:
        root = Path(directory)
        build_tree(root, documents)
        findings, _scanned = checker.check_tree(root)
        return findings


class CitationRuleTests(unittest.TestCase):
    """The rules the checker enforces, one fixture per rule."""

    def test_a_correct_citation_passes(self) -> None:
        """A citation that names a symbol that exists must not be rejected."""
        findings = findings_for(
            {
                "architecture/good.md": (
                    "The runner owns the launch seam: "
                    "`crates/demo/src/lib.rs::real_function`.\n"
                )
            }
        )
        self.assertEqual(findings, [], findings)

    def test_a_citation_naming_a_non_existent_symbol_fails(self) -> None:
        """A symbol that is not in the named file must be rejected."""
        findings = findings_for(
            {
                "architecture/bad.md": (
                    "A symbol that is not there: `crates/demo/src/lib.rs::no_such_symbol`.\n"
                )
            }
        )
        self.assertEqual(len(findings), 1, findings)
        self.assertEqual(findings[0].kind, "unresolved-symbol")
        self.assertIn("no_such_symbol", findings[0].detail)

    def test_a_citation_naming_a_non_existent_file_fails(self) -> None:
        """A path that is not in the tree must be rejected."""
        findings = findings_for(
            {
                "architecture/bad.md": (
                    "A file that is not there: `crates/demo/src/absent.rs::real_function`.\n"
                )
            }
        )
        self.assertEqual(len(findings), 1, findings)
        self.assertEqual(findings[0].kind, "unresolved-path")

    def test_a_legacy_line_number_citation_fails(self) -> None:
        """The legacy `file.rs:NNN` form must be a failure, not a warning."""
        findings = findings_for(
            {
                "architecture/bad.md": (
                    "A line number: `crates/demo/src/lib.rs:12`.\n"
                )
            }
        )
        self.assertEqual(len(findings), 1, findings)
        self.assertEqual(findings[0].kind, "legacy-line-citation")
        self.assertIn("12", findings[0].detail)

    def test_a_legacy_line_number_range_fails(self) -> None:
        """A line *range* is the same defect as a single line."""
        findings = findings_for(
            {
                "architecture/bad.md": "A line range: `crates/demo/src/lib.rs:12-40`.\n"
            }
        )
        self.assertEqual(len(findings), 1, findings)
        self.assertEqual(findings[0].kind, "legacy-line-citation")

    def test_a_bare_line_number_continuation_fails(self) -> None:
        """The `:1457` continuation form is a line number too."""
        findings = findings_for(
            {"architecture/bad.md": "Two lines: `crates/demo/src/lib.rs:12`, `:13`.\n"}
        )
        self.assertEqual(len(findings), 2, findings)
        self.assertEqual({finding.kind for finding in findings}, {"legacy-line-citation"})

    def test_an_ambiguous_bare_basename_fails(self) -> None:
        """`lib.rs` names four files; a citation must name the full path."""
        findings = findings_for(
            {"architecture/bad.md": "Ambiguous: `lib.rs::real_function`.\n"}
        )
        self.assertEqual(len(findings), 1, findings)
        self.assertEqual(findings[0].kind, "unresolved-path")
        self.assertIn("ambiguous", findings[0].detail)

    def test_a_unique_bare_basename_resolves(self) -> None:
        """`store.rs` names one file, so the bare basename is unambiguous."""
        findings = findings_for(
            {"architecture/good.md": "Unambiguous: `store.rs::store_function`.\n"}
        )
        self.assertEqual(findings, [], findings)

    def test_a_citation_into_a_python_file_resolves_python_symbols(self) -> None:
        """Python `def`, `class`, and module-level assignments are symbols."""
        for symbol in ("real_function", "RealClass", "REAL_CONSTANT", "RealClass::real_method"):
            with self.subTest(symbol=symbol):
                findings = findings_for(
                    {
                        "architecture/good.md": (
                            f"A Python symbol: `scripts/demo.py::{symbol}`.\n"
                        )
                    }
                )
                self.assertEqual(findings, [], findings)

    def test_a_python_symbol_that_does_not_exist_fails(self) -> None:
        """The Python symbol table is checked, not merely parsed."""
        findings = findings_for(
            {"architecture/bad.md": "Missing: `scripts/demo.py::not_there`.\n"}
        )
        self.assertEqual(len(findings), 1, findings)
        self.assertEqual(findings[0].kind, "unresolved-symbol")

    def test_every_required_rust_item_kind_is_recognised(self) -> None:
        """The plan names the Rust item kinds the checker must recognise."""
        for symbol in (
            "real_function",
            "REAL_CONSTANT",
            "REAL_STATIC",
            "RealStruct",
            "RealEnum",
            "RealEnum::First",
            "RealTrait",
            "RealTrait::trait_method",
            "RealStruct::real_method",
            "real_module",
            "real_module::module_function",
            "RealAlias",
            "real_macro",
        ):
            with self.subTest(symbol=symbol):
                findings = findings_for(
                    {
                        "architecture/good.md": (
                            f"A Rust item: `crates/demo/src/lib.rs::{symbol}`.\n"
                        )
                    }
                )
                self.assertEqual(findings, [], findings)

    def test_a_symbol_that_is_ambiguous_within_its_file_is_rejected(self) -> None:
        """Where a name is ambiguous in its file, the qualified path is required."""
        source = (
            "pub struct Alpha;\n"
            "pub struct Beta;\n"
            "impl Alpha { pub fn build() {} }\n"
            "impl Beta { pub fn build() {} }\n"
        )
        with tempfile.TemporaryDirectory(prefix="eggwork-citation-test-") as directory:
            root = Path(directory)
            build_tree(root, {"architecture/bad.md": "Ambiguous: `crates/demo/src/lib.rs::build`.\n"})
            (root / "crates" / "demo" / "src" / "lib.rs").write_text(
                source, encoding="utf-8"
            )
            findings, _scanned = checker.check_tree(root)
        self.assertEqual(len(findings), 1, findings)
        self.assertEqual(findings[0].kind, "unresolved-symbol")
        self.assertIn("ambiguous", findings[0].detail)

    def test_a_qualified_symbol_resolves_an_ambiguous_name(self) -> None:
        """Naming the qualified path is the documented remedy for ambiguity."""
        source = (
            "pub struct Alpha;\n"
            "pub struct Beta;\n"
            "impl Alpha { pub fn build() {} }\n"
            "impl Beta { pub fn build() {} }\n"
        )
        with tempfile.TemporaryDirectory(prefix="eggwork-citation-test-") as directory:
            root = Path(directory)
            build_tree(
                root,
                {"architecture/good.md": "Qualified: `crates/demo/src/lib.rs::Alpha::build`.\n"},
            )
            (root / "crates" / "demo" / "src" / "lib.rs").write_text(
                source, encoding="utf-8"
            )
            findings, _scanned = checker.check_tree(root)
        self.assertEqual(findings, [], findings)

    def test_a_citation_without_a_symbol_is_still_path_checked(self) -> None:
        """A path-only citation names a location, so the path must exist."""
        findings = findings_for(
            {"architecture/bad.md": "A missing path: `crates/demo/src/absent.rs`.\n"}
        )
        self.assertEqual(len(findings), 1, findings)
        self.assertEqual(findings[0].kind, "unresolved-path")

    def test_a_rust_path_expression_is_not_a_citation(self) -> None:
        """`ExecutionStore::open` names no location in this repository."""
        findings = findings_for(
            {"architecture/good.md": "A path expression: `ExecutionStore::open`.\n"}
        )
        self.assertEqual(findings, [], findings)

    def test_a_fenced_code_block_is_not_scanned(self) -> None:
        """A shell command in a code block is an instruction, not a citation."""
        findings = findings_for(
            {
                "architecture/good.md": (
                    "```bash\n"
                    "cargo build --locked -p demo\n"
                    "```\n"
                )
            }
        )
        self.assertEqual(findings, [], findings)

    def test_a_non_source_path_is_not_existence_checked(self) -> None:
        """Release artifacts and configuration files are not source citations."""
        findings = findings_for(
            {
                "architecture/good.md": (
                    "A release artifact: `release-manifest.json` and a manifest "
                    "`Cargo.toml`.\n"
                )
            }
        )
        self.assertEqual(findings, [], findings)

    def test_the_checker_reports_the_documents_it_scanned(self) -> None:
        """A document must not escape the gate by not being scanned."""
        with tempfile.TemporaryDirectory(prefix="eggwork-citation-test-") as directory:
            root = Path(directory)
            build_tree(
                root,
                {
                    "architecture/one.md": "Nothing to see.\n",
                    "docs/two.md": "Nothing to see.\n",
                },
            )
            findings, scanned = checker.check_tree(root)
            scanned_relative = [str(path.relative_to(root)) for path in scanned]
        self.assertEqual(findings, [], findings)
        self.assertEqual(scanned_relative, ["architecture/one.md", "docs/two.md"])

    def test_an_empty_document_set_fails_closed(self) -> None:
        """"Cannot check" must never be reported as "checked"."""
        with tempfile.TemporaryDirectory(prefix="eggwork-citation-test-") as directory:
            root = Path(directory)
            (root / "architecture").mkdir(parents=True)
            (root / "docs").mkdir(parents=True)
            findings, scanned = checker.check_tree(root)
        self.assertEqual(scanned, [])
        self.assertEqual(len(findings), 1)
        self.assertEqual(findings[0].kind, "no-documents")


class NegativeExitProofTests(unittest.TestCase):
    """The checker's own self-test, run as the CI job would run it."""

    def test_the_negative_exit_proof_passes(self) -> None:
        """`--prove-negative-exit` must prove the rejection path works."""
        result = subprocess.run(
            [sys.executable, str(CHECKER), "--prove-negative-exit"],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(
            result.returncode,
            0,
            "the negative-exit proof failed; the guard's failure path is not "
            f"proven\n{result.stdout}\n{result.stderr}",
        )
        self.assertIn("negative-exit proof passed", result.stdout)

    def test_the_negative_exit_proof_rejects_a_deliberately_wrong_citation(self) -> None:
        """The proof must fail if the checker stops rejecting a wrong citation."""
        with tempfile.TemporaryDirectory(prefix="eggwork-citation-test-") as directory:
            root = Path(directory)
            checker.build_fixture_tree(root)
            findings, _scanned = checker.check_tree(root)
        kinds = {finding.kind for finding in findings}
        self.assertIn("unresolved-symbol", kinds)
        self.assertIn("legacy-line-citation", kinds)
        self.assertIn("unresolved-path", kinds)


if __name__ == "__main__":
    unittest.main()
