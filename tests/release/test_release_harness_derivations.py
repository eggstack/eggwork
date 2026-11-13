"""Bounded tests for the qualification harness's derivations.

The rule this milestone establishes is that **multiple readers of one owner
are fine; multiple owners are not**. The harness therefore holds no literal
for any fact it asserts: the workspace version comes from the shared reader,
the install identities come from the producer release contract, and the
service identity comes from the candidate daemon.

These tests check that the harness really derives those facts, and — just as
importantly — that it cannot drift into remembering them again. The
"no literal" checks are the kind that fail when someone re-adds a constant,
which is the failure this milestone exists to prevent.
"""

from __future__ import annotations

import ast
import inspect
import os
import re
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "scripts"
CONTRACT = ROOT / "release" / "eggpack" / "distribution.toml"
DEPLOYMENT = ROOT / "crates" / "eggwork-server" / "src" / "deployment.rs"
CONTRACT_TEST = ROOT / "crates" / "eggwork-server" / "tests" / "release_contract.rs"

sys.path.insert(0, str(SCRIPTS))
import check_release_tag as gate  # noqa: E402
import qualify_release as harness  # noqa: E402
import release_candidate_probe as probe  # noqa: E402


def read_contract() -> dict[str, Any]:
    """The producer release contract, parsed the way the harness parses it."""
    return harness._read_contract(CONTRACT)


def contract_entries() -> dict[str, dict[str, Any]]:
    """The contract's targets, as the harness sees them."""
    return harness.contract_targets()


def rust_constants(source: str) -> dict[str, str]:
    """Read the `&str` constants out of a Rust source file.

    A cross-language check needs to read the other language's owner. The
    pattern is anchored on the whole declaration so a mention in prose or a
    different type cannot be mistaken for the constant.
    """
    found: dict[str, str] = {}
    for name, value in re.findall(
        r'pub const ([A-Z_]+): &str = "([^"]*)";', source
    ):
        found[name] = value
    return found


def rust_target_list(source: str, constant: str) -> list[str]:
    """Read one explicit target-triple list out of the Rust contract test."""
    match = re.search(
        rf"const {constant}: \[&str; \d+\] = \[([^\]]*)\];", source
    )
    if match is None:
        raise AssertionError(f"{constant} is not declared as an explicit list")
    return re.findall(r'"([^"]+)"', match.group(1))


def code_without_docstrings(path: Path) -> str:
    """The module's code, with prose docstrings removed.

    A docstring may name the product's own command — that is documentation,
    not a value the harness can assert. Everything else that remains is code,
    and a string literal in it is a value the harness holds.
    """
    tree = ast.parse(path.read_text(encoding="utf-8"))
    for node in ast.walk(tree):
        if not isinstance(node, (ast.Module, ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        body = node.body
        if (
            body
            and isinstance(body[0], ast.Expr)
            and isinstance(body[0].value, ast.Constant)
            and isinstance(body[0].value.value, str)
        ):
            body.pop(0)
    return ast.unparse(tree)


class NoRememberedFactsTest(unittest.TestCase):
    """The harness holds no literal for any fact it asserts."""

    def harness_code(self) -> str:
        return code_without_docstrings(SCRIPTS / "qualify_release.py")

    def test_the_harness_holds_no_literal_asset_count(self) -> None:
        source = (SCRIPTS / "qualify_release.py").read_text(encoding="utf-8")
        self.assertNotIn("QUALIFIED_ASSET_COUNT", source)
        code = self.harness_code()
        self.assertNotRegex(code, r"ASSET_COUNT\s*=")

    def test_the_harness_holds_no_literal_install_or_service_identity(self) -> None:
        contract = read_contract()
        identities: set[str] = set()
        for entry in contract.get("targets", []):
            asset = entry.get("asset", {})
            if asset.get("kind") == "direct":
                identities.add(str(asset.get("install", "")))
            else:
                for item in asset.get("entries", []):
                    identities.add(str(item.get("install", "")))
        constants = rust_constants(DEPLOYMENT.read_text(encoding="utf-8"))
        identities.add(constants["SERVICE_ID"])
        identities.add(constants["MEMBER_DAEMON"])
        identities.add(constants["MEMBER_HELPER"])
        identities.discard("")
        code = self.harness_code()
        for identity in sorted(identities):
            with self.subTest(identity=identity):
                self.assertNotIn(identity, code)

    def test_the_harness_does_not_classify_a_target_by_its_name(self) -> None:
        # The host's own triple is built from the host's platform, which is a
        # fact about this machine and not a classification of a release
        # target. What must not happen is a *target* being classified by the
        # shape of its name.
        triples = set(contract_entries())
        for function in (
            harness.target_daemon_install,
            harness.target_helper_install,
            harness.target_ships_helper,
            harness.target_install_identities,
        ):
            with self.subTest(function=function.__name__):
                source = inspect.getsource(function)
                self.assertNotIn("endswith(", source)
                self.assertNotIn("startswith(", source)
                for triple in triples:
                    self.assertNotIn(triple, source)

    def test_the_harness_does_not_derive_its_expected_version_from_the_tag(self) -> None:
        code = self.harness_code()
        self.assertNotIn("release_tag.lstrip", code)
        self.assertNotIn("release_tag[1:]", code)


class InstallIdentityTest(unittest.TestCase):
    """The install identities are read from the contract."""

    def test_every_target_resolves_its_daemon_identity_from_the_contract(self) -> None:
        for triple, entry in contract_entries().items():
            with self.subTest(triple=triple):
                self.assertEqual(
                    harness.target_daemon_install(triple), entry["installs"][0]
                )

    def test_every_target_resolves_its_helper_identity_from_the_contract(self) -> None:
        for triple, entry in contract_entries().items():
            with self.subTest(triple=triple):
                expected = entry["installs"][1] if entry["kind"] == "bundle" else None
                self.assertEqual(harness.target_helper_install(triple), expected)

    def test_an_unknown_target_is_refused(self) -> None:
        with self.assertRaises(harness.QualificationFailure):
            harness.target_daemon_install("riscv64-unknown-linux-gnu")

    def test_the_rust_deployment_unit_agrees_with_the_contract(self) -> None:
        constants = rust_constants(DEPLOYMENT.read_text(encoding="utf-8"))
        bundles = [
            entry
            for entry in contract_entries().values()
            if entry["kind"] == "bundle"
        ]
        self.assertTrue(bundles, "the contract publishes at least one bundle target")
        for entry in bundles:
            with self.subTest(triple=entry["triple"]):
                self.assertEqual(entry["installs"][0], constants["MEMBER_DAEMON"])
                self.assertEqual(entry["installs"][1], constants["MEMBER_HELPER"])

    def test_the_harness_reads_the_service_identity_from_the_daemon(self) -> None:
        source = (SCRIPTS / "qualify_release.py").read_text(encoding="utf-8")
        self.assertIn("def service_identity(", source)
        self.assertIn('"service",\n            "spec",', source)


class TargetClassificationTest(unittest.TestCase):
    """The target classification comes from the contract's asset entries."""

    def test_the_helper_ships_exactly_on_the_contract_bundle_targets(self) -> None:
        for triple, entry in contract_entries().items():
            with self.subTest(triple=triple):
                self.assertEqual(
                    harness.target_ships_helper(triple), entry["kind"] == "bundle"
                )

    def test_the_helper_ships_exactly_on_the_linux_release_targets(self) -> None:
        source = CONTRACT_TEST.read_text(encoding="utf-8")
        linux = set(rust_target_list(source, "LINUX_TARGETS"))
        darwin = set(rust_target_list(source, "DARWIN_TARGETS"))
        windows = set(rust_target_list(source, "WINDOWS_TARGETS"))
        shipping = {
            triple
            for triple in contract_entries()
            if harness.target_ships_helper(triple)
        }
        self.assertEqual(shipping, linux)
        for triple in sorted(darwin | windows):
            with self.subTest(triple=triple):
                self.assertFalse(harness.target_ships_helper(triple))

    def test_every_target_publishes_its_install_identities(self) -> None:
        for triple, entry in contract_entries().items():
            with self.subTest(triple=triple):
                self.assertEqual(
                    harness.target_install_identities(triple), entry["installs"]
                )


class AssetInventoryTest(unittest.TestCase):
    """The expected asset set is derived, and never counted from memory."""

    def contract_asset_names(self) -> set[str]:
        """The asset names the contract's own templates produce."""
        contract = read_contract()
        product = contract["product"]["id"]
        version = probe.workspace_version()
        names: set[str] = {"release-manifest.json"}
        for entry in contract.get("targets", []):
            triple = entry["triple"]
            asset = entry["asset"]
            templates = (
                [asset["asset"]]
                if asset["kind"] == "direct"
                else [item["asset"] for item in asset["entries"]]
            )
            for template in templates:
                name = (
                    template.replace("{product}", product)
                    .replace("{version}", version)
                    .replace("{target}", triple)
                )
                names.add(name)
                names.add(f"{name}.sha256")
        names.add("install.sh")
        return names

    def release_manifest(self) -> dict[str, Any]:
        """The release manifest the contract's templates describe."""
        contract = read_contract()
        product = contract["product"]["id"]
        version = probe.workspace_version()
        targets = []
        for entry in contract.get("targets", []):
            triple = entry["triple"]
            asset = entry["asset"]

            def render(template: str) -> str:
                return (
                    template.replace("{product}", product)
                    .replace("{version}", version)
                    .replace("{target}", triple)
                )

            if asset["kind"] == "direct":
                form = {
                    "kind": "direct",
                    "artifact": {"name": render(asset["asset"]), "size": 1},
                }
            else:
                form = {
                    "kind": "bundle",
                    "entries": [
                        {"artifact": {"name": render(item["asset"]), "size": 1}}
                        for item in asset["entries"]
                    ],
                }
            targets.append({"target": triple, "form": form})
        return {"release_id": "v0.0.0", "product_id": product, "targets": targets}

    def test_the_expected_asset_set_equals_the_contract_derived_set(self) -> None:
        manifest = self.release_manifest()
        names = {name: {"size": 1} for name in self.contract_asset_names()}
        self.assertEqual(harness.expected_asset_names(manifest, names), set(names))

    def test_the_expected_asset_set_grows_with_the_contract(self) -> None:
        manifest = self.release_manifest()
        names = {name: {"size": 1} for name in self.contract_asset_names()}
        derived = harness.expected_asset_names(manifest, names)
        # Every published binary contributes its own sidecar, so the derived
        # set is larger than the binary count by exactly that many names.
        binaries = {
            name for name in derived if name.endswith(".sha256")
        }
        self.assertTrue(binaries)
        for name in binaries:
            self.assertIn(name[: -len(".sha256")], derived)


class VersionAuthorityTest(unittest.TestCase):
    """Which number is the build assertion, and which is the release gate."""

    def test_the_harness_expected_version_is_the_manifest_version(self) -> None:
        manifest = probe.read_manifest()
        package = manifest["workspace"]["package"]
        self.assertEqual(probe.workspace_version(), package["version"])
        self.assertEqual(harness.probe.workspace_version(), package["version"])

    def test_a_tag_that_disagrees_with_the_manifest_is_refused_by_the_gate(self) -> None:
        version = probe.workspace_version()
        self.assertEqual(gate.tag_failures(f"v{version}"), [])
        failures = gate.tag_failures("v0.0.0-not-the-version")
        self.assertTrue(failures, "a tag that names another version must be refused")
        self.assertIn(version, failures[0])

    def test_the_harness_delegates_the_tag_comparison_to_the_gate(self) -> None:
        source = inspect.getsource(harness.stage_install)
        self.assertIn("tag_guard.tag_failures(", source)
        self.assertNotIn("release_tag.lstrip", source)
        self.assertNotIn("release_tag[1:]", source)


@unittest.skipUnless(os.name == "posix", "the stub daemon is a POSIX script")
class ServiceIdentityTest(unittest.TestCase):
    """The service identity is read from the daemon, not remembered."""

    def stub_daemon(self, root: Path, body: str) -> Path:
        stub = root / "daemon"
        stub.write_text(f"#!{sys.executable}\n{body}\n", encoding="utf-8")
        stub.chmod(0o755)
        return stub

    def test_the_identity_is_the_daemons_to_report(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stub = self.stub_daemon(
                root,
                "import json\n"
                "print(json.dumps({'schema_version': 1, "
                "'service_id': 'a-service-identity-from-the-daemon'}))",
            )
            config = root / "node.json"
            config.write_text("{}", encoding="utf-8")
            self.assertEqual(
                harness.service_identity(stub, config),
                "a-service-identity-from-the-daemon",
            )

    def test_a_daemon_that_reports_no_identity_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stub = self.stub_daemon(root, "print('{}')")
            config = root / "node.json"
            config.write_text("{}", encoding="utf-8")
            with self.assertRaises(harness.QualificationFailure):
                harness.service_identity(stub, config)


if __name__ == "__main__":
    unittest.main()
