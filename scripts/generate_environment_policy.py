#!/usr/bin/env python3
"""Generate the sandbox environment-policy copies from the runner's source of truth.

The node's environment policy — the denied-key list and its matching rule, the
node-owned (reserved) keys, the node-owned baseline, and the `PATH` value
contract — is authored once, in
``crates/eggwork-runner/policy/environment-policy.toml``.

``eggwork-sandbox-helper`` enforces that policy and may not depend on any
workspace crate, so the policy cannot be shared as a library. It is shared as
a *generated copy*: this script writes one copy into each crate, inside that
crate's own source directory, and both crates compile only their own copy. No
crate reads a file from outside its own directory, no ``build.rs`` is added,
and no dependency edge is created.

The generator is not the gate. The *test* is: a parity test compares the two
copies and fails the build on a one-sided edit, whether or not this script was
ever run. This script exists so that the intended edit is a one-step
operation.

Usage::

    python3 scripts/generate_environment_policy.py           # rewrite both copies
    python3 scripts/generate_environment_policy.py --check   # fail if they differ
    python3 scripts/generate_environment_policy.py --root DIR # operate on another tree

The output is deterministic: re-running over an unchanged source of truth
produces byte-identical files. The script never touches the network, uses only
the standard library, and fails loudly — without writing anything — when the
source of truth is malformed.
"""

from __future__ import annotations

import argparse
import os
import re
import sys
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

POLICY_RELATIVE = Path("crates/eggwork-runner/policy/environment-policy.toml")
CORE_RELATIVE = Path("crates/eggwork-core/src/lib.rs")
RUNNER_OUTPUT_RELATIVE = Path("crates/eggwork-runner/src/environment_policy.rs")
HELPER_OUTPUT_RELATIVE = Path("crates/eggwork-sandbox-helper/src/environment_policy.rs")

# The five protocol limit constants the helper used to re-declare by hand.
# `eggwork-core` owns the wire limits; the helper cannot import them, so the
# generator reads them out of the owner and emits them into the helper's copy.
CORE_LIMIT_NAMES = (
    "MAX_ARG_COUNT",
    "MAX_ARG_BYTES",
    "MAX_ENV_COUNT",
    "MAX_ENV_NAME_BYTES",
    "MAX_ENV_VALUE_BYTES",
)

# The one matching rule this generator knows how to emit. A new rule is a
# contract change and needs a new emitter, not a silently different predicate.
EXPECTED_MATCH_RULE = "ascii-uppercase-exact-or-prefix"

# The only `PATH` scope this generator knows about.
EXPECTED_PATH_SCOPE = "linux-unix"

# `pub const NAME: usize = <expression>;` in `eggwork-core`.
CORE_CONST_PATTERN = re.compile(
    r"^pub const (?P<name>MAX_[A-Z0-9_]+): usize = (?P<value>[0-9_ ()\-+*/]+);$",
    re.MULTILINE,
)


class PolicyError(Exception):
    """The source of truth is malformed. Nothing was written."""


def fail(message: str) -> "NoReturn":  # noqa: F821 - typing only
    raise PolicyError(message)


# --------------------------------------------------------------------------
# A tiny integer-expression evaluator.
#
# `eggwork-core` writes some limits as arithmetic (`32 * 1024`), so the
# generator has to read an expression rather than a literal. It deliberately
# does not use `eval`: the parser below accepts nothing but integer literals,
# `+ - * /` and parentheses, and it refuses anything else.
# --------------------------------------------------------------------------

_EXPRESSION_TOKEN = re.compile(r"\s*(?:(?P<number>[0-9]+)|(?P<operator>[-+*/()]))")


def evaluate_integer_expression(expression: str) -> int:
    """Evaluate a pure integer arithmetic expression, or fail loudly."""
    tokens: list[str] = []
    position = 0
    while position < len(expression):
        match = _EXPRESSION_TOKEN.match(expression, position)
        if match is None:
            fail(
                f"the limit expression {expression!r} contains something that is not an "
                f"integer literal or an arithmetic operator"
            )
        position = match.end()
        tokens.append(match.group("number") if match.group("number") else match.group("operator"))
    if not tokens:
        fail(f"the limit expression {expression!r} is empty")

    index = 0

    def parse_primary() -> int:
        nonlocal index
        if index >= len(tokens):
            fail(f"the limit expression {expression!r} ends unexpectedly")
        token = tokens[index]
        if token == "(":
            index += 1
            value = parse_sum()
            if index >= len(tokens) or tokens[index] != ")":
                fail(f"the limit expression {expression!r} has an unbalanced parenthesis")
            index += 1
            return value
        if not token.isdigit():
            fail(f"the limit expression {expression!r} has an unexpected token {token!r}")
        index += 1
        return int(token)

    def parse_product() -> int:
        nonlocal index
        value = parse_primary()
        while index < len(tokens) and tokens[index] in ("*", "/"):
            operator = tokens[index]
            index += 1
            right = parse_primary()
            if operator == "*":
                value *= right
            else:
                if right == 0:
                    fail(f"the limit expression {expression!r} divides by zero")
                value //= right
        return value

    def parse_sum() -> int:
        nonlocal index
        value = parse_product()
        while index < len(tokens) and tokens[index] in ("+", "-"):
            operator = tokens[index]
            index += 1
            right = parse_product()
            value = value + right if operator == "+" else value - right
        return value

    result = parse_sum()
    if index != len(tokens):
        fail(f"the limit expression {expression!r} has trailing tokens")
    return result


def read_core_limits(path: Path) -> dict[str, int]:
    """Read the five protocol limit constants out of `eggwork-core`."""
    try:
        source = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        fail(f"the owner of the wire limits is missing: {path}")
    except OSError as error:
        fail(f"the owner of the wire limits could not be read: {path}: {error}")

    found: dict[str, int] = {}
    for match in CORE_CONST_PATTERN.finditer(source):
        name = match.group("name")
        if name in CORE_LIMIT_NAMES:
            found[name] = evaluate_integer_expression(match.group("value"))
    missing = [name for name in CORE_LIMIT_NAMES if name not in found]
    if missing:
        fail(
            f"{path} does not export the protocol limit constants "
            f"{', '.join(missing)}; the helper's copy of them would be guessed rather "
            f"than generated"
        )
    return found


# --------------------------------------------------------------------------
# Reading and validating the source of truth.
# --------------------------------------------------------------------------


def _lookup(document: dict, path: str) -> object:
    """Look up a dotted path such as `baseline.windows_common`."""
    node: object = document
    for part in path.split("."):
        if not isinstance(node, dict) or part not in node:
            fail(f"the environment policy has no [{path}] table")
        node = node[part]
    return node


def _require_table(document: dict, name: str) -> dict:
    value = _lookup(document, name)
    if not isinstance(value, dict):
        fail(f"the environment policy has no [{name}] table")
    return value


def _require_string_list(document: dict, table: str, name: str) -> list[str]:
    values = _require_table(document, table).get(name)
    if not isinstance(values, list) or not values:
        fail(f"the environment policy's [{table}].{name} must be a non-empty list")
    for value in values:
        if not isinstance(value, str) or not value:
            fail(f"the environment policy's [{table}].{name} must hold non-empty strings")
    if len(set(values)) != len(values):
        duplicates = sorted({value for value in values if values.count(value) > 1})
        fail(
            f"the environment policy's [{table}].{name} repeats "
            f"{', '.join(duplicates)}; a repeated entry is a contract change, not a duplicate"
        )
    return list(values)


def _require_baseline_table(document: dict, name: str) -> list[tuple[str, str]]:
    table = document.get("baseline", {})
    if not isinstance(table, dict):
        fail("the environment policy has no [baseline] table")
    entries = table.get(name)
    if not isinstance(entries, dict) or not entries:
        fail(f"the environment policy has no non-empty [baseline.{name}] table")
    pairs: list[tuple[str, str]] = []
    for key, value in entries.items():
        if not isinstance(key, str) or not key:
            fail(f"the environment policy's [baseline.{name}] has an empty key")
        if not isinstance(value, str):
            fail(f"the environment policy's [baseline.{name}].{key} must be a string")
        pairs.append((key, value))
    return pairs


def read_policy(path: Path) -> dict:
    """Read and validate the source of truth. Returns the validated policy."""
    try:
        with path.open("rb") as handle:
            document = tomllib.load(handle)
    except FileNotFoundError:
        fail(f"the environment policy source of truth is missing: {path}")
    except tomllib.TOMLDecodeError as error:
        fail(f"the environment policy source of truth is not valid TOML: {path}: {error}")
    except OSError as error:
        fail(f"the environment policy source of truth could not be read: {path}: {error}")

    version = document.get("schema_version")
    if version != 1:
        fail(
            f"the environment policy source of truth declares schema_version {version!r}; "
            f"this generator only understands version 1"
        )

    match = _require_table(document, "match")
    rule = match.get("rule")
    if rule != EXPECTED_MATCH_RULE:
        fail(
            f"the environment policy's [match].rule is {rule!r}; this generator only "
            f"emits {EXPECTED_MATCH_RULE!r}"
        )

    denied = _require_string_list(document, "denied", "keys")
    reserved = _require_string_list(document, "reserved", "keys")
    unix = _require_baseline_table(document, "unix")
    windows_common = _require_string_list(document, "baseline.windows_common", "keys")

    path_contract = _require_table(document, "path_contract")
    path_value = path_contract.get("value")
    if not isinstance(path_value, str) or not path_value:
        fail("the environment policy's [path_contract].value must be a non-empty string")
    scope = path_contract.get("scope")
    if scope != EXPECTED_PATH_SCOPE:
        fail(
            f"the environment policy's [path_contract].scope is {scope!r}; this generator "
            f"only emits {EXPECTED_PATH_SCOPE!r}"
        )

    unix_values = dict(unix)
    for key in windows_common:
        if key not in unix_values:
            fail(
                f"the environment policy's [baseline.windows_common] names {key}, which the "
                f"Unix baseline does not carry; the Windows branch's shared keys must exist "
                f"in the Unix baseline"
            )
    common = [(key, unix_values[key]) for key in windows_common]

    baseline = unix
    baseline_keys = [key for key, _ in baseline]
    if len(set(baseline_keys)) != len(baseline_keys):
        fail("the environment policy's baseline names the same key twice")
    if set(reserved) != set(baseline_keys):
        only_reserved = sorted(set(reserved) - set(baseline_keys))
        only_baseline = sorted(set(baseline_keys) - set(reserved))
        fail(
            "the environment policy's node-owned class and its baseline disagree: "
            f"reserved-only {only_reserved}, baseline-only {only_baseline}. The node-owned "
            f"class is the node's own baseline, so the two cannot differ"
        )
    if reserved != baseline_keys:
        fail(
            "the environment policy's node-owned class and its baseline list the same keys in "
            "different orders; they are one list, written once, and the order is part of the "
            "generated contract"
        )
    unix_path = unix_values.get("PATH")
    if unix_path != path_value:
        fail(
            f"the environment policy's [path_contract].value ({path_value!r}) differs from "
            f"the Unix baseline's PATH ({unix_path!r}); the contract and the baseline are "
            f"the same value and are written twice"
        )

    return {
        "match_rule": rule,
        "denied": denied,
        "reserved": reserved,
        "baseline_common": common,
        "baseline": baseline,
        "path_value": path_value,
    }


# --------------------------------------------------------------------------
# Rendering.
# --------------------------------------------------------------------------


def rust_string(value: str) -> str:
    escaped = value.replace("\\", "\\\\").replace('"', '\\"')
    return f'"{escaped}"'


def rust_string_array(name: str, doc: list[str], values: list[str]) -> str:
    lines = [f"pub const {name}: &[&str] = &["]
    lines.extend(f"    {rust_string(value)}," for value in values)
    lines.append("];")
    return "\n".join([*doc, *lines])


def rust_pair_array(name: str, doc: list[str], values: list[tuple[str, str]]) -> str:
    lines = [f"pub const {name}: &[(&str, &str)] = &["]
    lines.extend(f"    ({rust_string(key)}, {rust_string(value)})," for key, value in values)
    lines.append("];")
    return "\n".join([*doc, *lines])


def module_header(crate_role: str) -> str:
    return "\n".join(
        [
            "// @generated by scripts/generate_environment_policy.py — do not edit.",
            "//",
            "// Source of truth: crates/eggwork-runner/policy/environment-policy.toml",
            "// Generator:      scripts/generate_environment_policy.py",
            "//",
            "// This file is generated. A hand edit is a contract change, not a local",
            "// fix: change the source of truth and re-run the generator.",
            "//",
            f"// This is the {crate_role}'s copy. The other copy is generated from the same",
            "// source of truth, and a parity test pins the two together, so a one-sided",
            "// edit fails the build rather than silently changing what is enforced.",
        ]
    )


def render_policy_items(policy: dict, role: str) -> str:
    """The policy items both crates carry, rendered identically in each."""
    parts = [
        module_header(role),
        "",
        "/// The keys a caller may not set.",
        "///",
        "/// The runner drops one of these silently: it is the request-normalisation",
        "/// boundary, and a hardened environment has to be a property of the node",
        "/// rather than something the caller has to get right. The helper refuses the",
        "/// whole launch when one of these is present: it is the enforcement point, and",
        "/// a denied key reaching it means the node's own filtering did not happen.",
        "/// The difference is deliberate and is the whole two-layer contract; only the",
        "/// list is shared, never the behaviour.",
        rust_string_array(
            "DENIED_KEYS",
            [],
            policy["denied"],
        ),
        "",
        "/// The rule used to match an environment key against `DENIED_KEYS` and against",
        "/// the node-owned class: ASCII-uppercase the key, then match it as an exact",
        "/// match or as a prefix.",
        "///",
        "/// The rule is part of the contract, not an implementation detail: the two",
        "/// copies must agree on it, or the runner would drop a key the helper still",
        "/// refuses.",
        f"pub const MATCH_RULE: &str = {rust_string(policy['match_rule'])};",
        "",
        "/// The node-owned (reserved) keys on the Unix baseline.",
        "///",
        "/// A caller-supplied value for one of these is discarded in favour of the",
        "/// node's value, so the node's baseline is what reaches the child and a",
        "/// launch specification can never carry a duplicate of a baseline key.",
        "///",
        "/// The class is the node-owned baseline, and the baseline is platform-shaped:",
        "/// on the Windows baseline the node owns `SystemRoot`, `windir` and `PATH`",
        "/// instead of the Unix locale keys, and that branch is runner-local.",
        rust_string_array(
            "RESERVED_KEYS",
            [],
            policy["reserved"],
        ),
        "",
        "/// The node-owned baseline keys every platform carries.",
        "///",
        "/// The values are the node's own; a caller cannot move them. This is the",
        "/// part of the Unix baseline the runner's Windows branch also installs, so",
        "/// the Windows branch does not have to re-list it.",
        rust_pair_array(
            "BASELINE_COMMON",
            [],
            policy["baseline_common"],
        ),
        "",
        "/// The node-owned Unix baseline, in the order the node installs it.",
        "///",
        "/// The Windows baseline is deliberately not here: its values are read from the",
        "/// parent process at spawn time, and the sandbox helper is Linux-only so it",
        "/// never sees a Windows launch. That branch is runner-local.",
        rust_pair_array(
            "BASELINE",
            [],
            policy["baseline"],
        ),
        "",
        "/// The enforced `PATH` value on the Linux/Unix contract.",
        "///",
        "/// A `PATH` that points outside this value widens what a confined target can",
        "/// execute, and the helper's Landlock ruleset does not restrict `execve` by",
        "/// path. The helper refuses a launch whose `PATH` differs; the runner",
        "/// guarantees the value in the child environment and in the launch",
        "/// specification.",
        "///",
        "/// The contract is Linux/Unix. The Windows baseline is explicitly outside it,",
        "/// so the Windows `PATH` must not be \"fixed\" to this value.",
        f"pub const PATH_VALUE: &str = {rust_string(policy['path_value'])};",
        "",
        "/// True when `key` is denied: the caller may not set it.",
        "///",
        "/// The key is ASCII-uppercased first, so the match is case-insensitive, and",
        "/// the entry is matched as an exact match or as a prefix.",
        "pub fn denied_environment_key(key: &str) -> bool {",
        "    let upper = key.to_ascii_uppercase();",
        "    DENIED_KEYS",
        "        .iter()",
        "        .any(|denied| upper == *denied || upper.starts_with(denied))",
        "}",
        "",
        "/// True when `key` is node-owned on the Unix baseline.",
        "///",
        "/// The key is ASCII-uppercased first, so a caller's differently-cased spelling",
        "/// of a node-owned key is recognised as the same key.",
        "pub fn is_reserved_key(key: &str) -> bool {",
        "    let upper = key.to_ascii_uppercase();",
        "    RESERVED_KEYS.iter().any(|reserved| upper == *reserved)",
        "}",
        "",
        "/// The node's baseline entry for `key`, matched case-insensitively.",
        "///",
        "/// The returned name is the node's own spelling of the key, so a caller that",
        "/// used a different spelling can be told apart from one that used the node's.",
        "pub fn baseline_entry(key: &str) -> Option<(&'static str, &'static str)> {",
        "    let upper = key.to_ascii_uppercase();",
        "    BASELINE",
        "        .iter()",
        "        .find(|(name, _)| *name == upper)",
        "        .map(|(name, value)| (*name, *value))",
        "}",
        "",
        "/// The node's baseline value for `key`, matched case-insensitively.",
        "///",
        "/// The value is the node's own; a caller's value for a node-owned key is",
        "/// discarded in favour of it.",
        "pub fn baseline_value(key: &str) -> Option<&'static str> {",
        "    baseline_entry(key).map(|(_, value)| value)",
        "}",
    ]
    return "\n".join(parts) + "\n"


def render_runner_module(policy: dict) -> str:
    return render_policy_items(policy, "runner")


def render_helper_module(policy: dict, limits: dict[str, int]) -> str:
    parts = [
        render_policy_items(policy, "sandbox helper").rstrip("\n"),
        "",
        "/// The protocol limit constants, generated from `eggwork-core`.",
        "///",
        "/// The helper is a standalone binary with no workspace-crate dependency, so it",
        "/// cannot import `eggwork_core::MAX_ARG_COUNT` and friends. The generator reads",
        "/// these values out of the owner of the wire limits, so the helper's copy is",
        "/// generated rather than hand-maintained, and a parity test compares them",
        "/// against `eggwork-core` at run time.",
        "///",
        "/// `MAX_SPEC_BYTES` is helper-only, has no core counterpart, and stays in the",
        "/// helper's own source.",
    ]
    for name in CORE_LIMIT_NAMES:
        parts.append(f"pub const {name}: usize = {limits[name]};")
    return "\n".join(parts) + "\n"


# --------------------------------------------------------------------------
# Writing.
# --------------------------------------------------------------------------


def write_atomically(path: Path, content: str) -> None:
    """Write `content` to `path` without ever leaving a partial file behind."""
    path.parent.mkdir(parents=True, exist_ok=True)
    handle, temporary = tempfile.mkstemp(dir=str(path.parent), prefix=".environment-policy-")
    try:
        with os.fdopen(handle, "w", encoding="utf-8", newline="\n") as stream:
            stream.write(content)
        os.replace(temporary, path)
    except BaseException:
        try:
            os.unlink(temporary)
        except OSError:
            pass
        raise


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--check",
        action="store_true",
        help="do not write anything; fail when the checked-in copies differ from the source of truth",
    )
    parser.add_argument(
        "--root",
        type=Path,
        default=ROOT,
        help="the repository root to operate on (default: the tree this script lives in)",
    )
    arguments = parser.parse_args(argv)
    root = arguments.root.resolve()

    policy_path = root / POLICY_RELATIVE
    core_path = root / CORE_RELATIVE
    runner_output = root / RUNNER_OUTPUT_RELATIVE
    helper_output = root / HELPER_OUTPUT_RELATIVE

    try:
        policy = read_policy(policy_path)
        limits = read_core_limits(core_path)
    except PolicyError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1

    rendered = {
        runner_output: render_runner_module(policy),
        helper_output: render_helper_module(policy, limits),
    }

    if arguments.check:
        stale: list[Path] = []
        for path, content in rendered.items():
            try:
                current = path.read_text(encoding="utf-8")
            except OSError:
                stale.append(path)
                continue
            if current != content:
                stale.append(path)
        if stale:
            for path in stale:
                print(
                    f"error: {path.relative_to(root)} is not what the generator produces; "
                    f"run `python3 scripts/generate_environment_policy.py`",
                    file=sys.stderr,
                )
            return 1
        print("the environment-policy copies are up to date")
        return 0

    for path, content in rendered.items():
        write_atomically(path, content)
        print(f"wrote {path.relative_to(root)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
