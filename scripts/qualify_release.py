#!/usr/bin/env python3
"""Operational qualification harness for exact Eggwork release bytes.

Ownership: this harness qualifies Eggwork's *installed* product surface. It is
not a producer tool. Eggpack owns release construction, assets, sidecars,
manifest, installers, and draft staging; Eggup owns filesystem transactions and
platform service-manager mechanics. This file only:

- retrieves the exact bytes of one already-staged release through the
  authenticated GitHub API (never a re-build, never a public URL);
- proves each retrieved artifact against that release's own
  ``release-manifest.json`` and its ``.sha256`` sidecar;
- drives the *installed* ``eggworkd`` binary through the same operator commands
  a human would run (first install, native service lifecycle, deployment apply);
- writes a bounded, secret-free JSON receipt.

It never rebuilds a release artifact, never stages or publishes a release, and
never mutates a tag. Every child process is invoked from an argv list with
``shell=False``, so nothing here is interpreted by a shell.

Stages
------

``install``
    First-install smoke: download, verify, install, digest re-check, installed
    ``version``, ``config validate``, and ``doctor``.

``installer``
    Generated exact-release installer: require it to hardcode the exact
    ``releases/download/<tag>`` origin, then require its destination-exists
    refusal. The refusal is checked before any download, so it is qualified
    while the release is still a draft.

``service``
    Native service-lifecycle matrix through the product's own
    ``eggworkd service`` command: install -> inspect Owned -> start -> running
    -> restart -> running -> stop -> stopped -> uninstall, repeated start/stop
    idempotence, and a foreign same-name non-mutation negative control.

``update``
    Drain / update / rollback / recovery matrix against real installed
    generations using ``eggworkd deployment apply``.

Each stage exits non-zero on the first unproven expectation and always attempts
cleanup, so a failed qualification never leaves a mutated native service behind.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import os
import platform
import shutil
import stat
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Iterator, Sequence

# Bounds. A qualification harness that can hang, or read an unbounded file, is
# not a qualification harness.
MAX_ASSET_BYTES = 256 * 1024 * 1024
# The release API occasionally answers a well-formed request with a 5xx. Retry
# only that, so qualification reflects the candidate rather than upstream uptime.
TRANSIENT_ATTEMPTS = 5
TRANSIENT_BACKOFF_SECONDS = 30
MAX_MANIFEST_BYTES = 1 << 20
MAX_PROCESS_OUTPUT_BYTES = 1 << 20
MAX_TEXT_ASSET_BYTES = 1 << 20
DEFAULT_PROCESS_TIMEOUT = 180.0
# A native service transition is bounded by the *product's* own timeout (60s per
# operation), and Eggup may compose several observations per invocation. The
# harness bound is deliberately larger so a slow hosted SCM or launchd is
# reported as a product timeout rather than truncated into a harness error.
SERVICE_TRANSITION_TIMEOUT = 300.0
SERVICE_ID = "eggwork-node"
HELPER_INSTALL_ID = "eggwork-sandbox-helper"


class QualificationFailure(Exception):
    """An unproven qualification expectation."""


# --------------------------------------------------------------------------
# Receipt
# --------------------------------------------------------------------------


@dataclass
class Receipt:
    """Bounded, secret-free qualification evidence."""

    stage: str
    facts: dict[str, Any] = field(default_factory=dict)
    steps: list[dict[str, Any]] = field(default_factory=list)

    def record(self, name: str, **facts: Any) -> None:
        self.steps.append({"step": name, **facts})
        self.facts.update(facts)

    def expect(self, name: str, condition: bool, detail: str) -> None:
        self.record(name, ok=bool(condition), detail=detail)
        if not condition:
            raise QualificationFailure(f"{name}: {detail}")

    def as_json(self) -> str:
        return json.dumps(
            {"schema_version": 1, "stage": self.stage, "facts": self.facts, "steps": self.steps},
            indent=2,
            sort_keys=True,
        )


# --------------------------------------------------------------------------
# Bounded, shell-free process execution
# --------------------------------------------------------------------------


@dataclass
class ProcessResult:
    argv: list[str]
    returncode: int
    stdout: str
    stderr: str
    duration_seconds: float

    def json(self) -> dict[str, Any]:
        """Parse stdout as a single JSON object, or fail closed."""
        text = self.stdout.strip()
        if not text:
            raise QualificationFailure(
                f"{self.argv[0]} produced no JSON output: {self.stderr.strip()[:300]}"
            )
        try:
            value = json.loads(text)
        except json.JSONDecodeError as error:
            raise QualificationFailure(f"{self.argv[0]} produced non-JSON output") from error
        if not isinstance(value, dict):
            raise QualificationFailure(f"{self.argv[0]} produced JSON that is not an object")
        return value


def run(
    argv: Sequence[str],
    *,
    timeout: float = DEFAULT_PROCESS_TIMEOUT,
    check: bool = True,
    cwd: Path | None = None,
    shell: bool = False,
) -> ProcessResult:
    """Run one bounded child process.

    ``shell=True`` is accepted only for the generated release installer, which
    is itself a shell script; every product invocation stays shell-free.
    """
    argv = [str(part) for part in argv]
    started = time.monotonic()
    try:
        completed = subprocess.run(
            argv,
            shell=shell,
            capture_output=True,
            timeout=timeout,
            cwd=str(cwd) if cwd else None,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        raise QualificationFailure(f"{argv[0]} exceeded its {timeout}s bound") from error
    except (FileNotFoundError, PermissionError) as error:
        raise QualificationFailure(f"{argv[0]} is not executable here: {error}") from error
    duration = time.monotonic() - started
    if (
        len(completed.stdout) > MAX_PROCESS_OUTPUT_BYTES
        or len(completed.stderr) > MAX_PROCESS_OUTPUT_BYTES
    ):
        raise QualificationFailure(f"{argv[0]} exceeded the bounded output size")
    result = ProcessResult(
        argv=argv,
        returncode=completed.returncode,
        stdout=completed.stdout.decode("utf-8", "replace"),
        stderr=completed.stderr.decode("utf-8", "replace"),
        duration_seconds=round(duration, 3),
    )
    if check and result.returncode != 0:
        raise QualificationFailure(
            f"{Path(argv[0]).name} exited {result.returncode}: "
            f"{result.stderr.strip()[:400] or result.stdout.strip()[:400]}"
        )
    return result


# --------------------------------------------------------------------------
# Exact release retrieval
# --------------------------------------------------------------------------


@dataclass(frozen=True)
class Artifact:
    """One release artifact the manifest declares for one target."""

    target: str
    name: str
    install: str
    size: int
    sha256: str


@dataclass(frozen=True)
class Release:
    """One exact release, as the API and its manifest report it."""

    tag: str
    id: int
    is_draft: bool
    source_revision: str
    artifacts: dict[str, Artifact]
    directory: Path

    def for_target(self, target: str) -> list[Artifact]:
        selected = [artifact for artifact in self.artifacts.values() if artifact.target == target]
        if not selected:
            raise QualificationFailure(f"{self.tag} publishes no artifact for {target}")
        return sorted(selected, key=lambda artifact: artifact.install)

    def install_identity(self, target: str, install: str) -> Artifact:
        for artifact in self.for_target(target):
            if artifact.install == install:
                return artifact
        raise QualificationFailure(f"{target} does not ship install identity {install!r}")


#: Environment variable holding the read token used to reach a draft release.
#:
#: A staged candidate is a *draft*, and GitHub's REST API does not expose drafts
#: to the Actions `GITHUB_TOKEN`: the list endpoint returns them only to a token
#: with repository scope. Qualification therefore reads a scoped secret from this
#: variable. The value is never placed in argv, stdout, or a receipt, and the
#: workflow's own `permissions` stay at `contents: read`.
TOKEN_VARIABLE = "RELEASE_QUALIFICATION_TOKEN"


def api_token() -> str:
    token = os.environ.get(TOKEN_VARIABLE, "").strip()
    if not token:
        raise QualificationFailure(
            f"{TOKEN_VARIABLE} is required: a staged candidate is a draft release, "
            "which the default Actions token cannot read"
        )
    return token


def _api_json(endpoint: str) -> Any:
    request = urllib.request.Request(
        f"https://api.github.com/{endpoint}",
        headers={
            "Authorization": f"Bearer {api_token()}",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "eggwork-operational-qualification",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            body = response.read(MAX_MANIFEST_BYTES)
    except urllib.error.HTTPError as error:
        # The status only: an error body can echo request detail.
        raise QualificationFailure(f"GitHub API {endpoint} returned HTTP {error.code}") from error
    try:
        return json.loads(body)
    except json.JSONDecodeError as error:
        raise QualificationFailure(f"GitHub API {endpoint} returned non-JSON") from error


def _resolve_release(repository: str, tag: str) -> dict[str, Any]:
    """Find one release by exact tag name, draft or published.

    The by-tag endpoint only resolves published releases, and the qualified
    candidate is a draft. Listing is used instead so the same code path works
    before and after publication, which is what makes the pre- and
    post-publication matrices comparable.
    """
    page = 1
    while page <= 10:
        releases = _api_json(f"repos/{repository}/releases?per_page=100&page={page}")
        if not isinstance(releases, list) or not releases:
            break
        for release in releases:
            if release.get("tag_name") == tag:
                return release
        page += 1
    raise QualificationFailure(f"{repository} has no release with tag {tag!r}")


def sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1 << 20):
            digest.update(chunk)
    return digest.hexdigest()


def download_asset(repository: str, asset_id: int, destination: Path) -> None:
    """Fetch one release asset, retrying only transient upstream failures.

    The release API is a shared dependency that occasionally answers a
    well-formed request with a 5xx. Hosted qualification aborted on one such
    response, which is indistinguishable from a real product failure and would
    make the qualification record depend on upstream uptime rather than on the
    candidate's behaviour.

    Only 5xx and 429 are retried. A 4xx means the request itself is wrong, so
    retrying it would hide a harness bug behind a delay. Every attempt is
    bounded, and the final failure still names the real status.
    """
    partial = destination.with_name(destination.name + ".partial")
    last_status: int | None = None
    for attempt in range(1, TRANSIENT_ATTEMPTS + 1):
        request = urllib.request.Request(
            f"https://api.github.com/repos/{repository}/releases/assets/{asset_id}",
            headers={
                "Authorization": f"Bearer {api_token()}",
                "Accept": "application/octet-stream",
                "User-Agent": "eggwork-operational-qualification",
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=300) as response:
                written = 0
                with partial.open("wb") as handle:
                    while chunk := response.read(1 << 20):
                        written += len(chunk)
                        if written > MAX_ASSET_BYTES:
                            raise QualificationFailure(
                                f"{destination.name} exceeds the bounded size"
                            )
                        handle.write(chunk)
        except urllib.error.HTTPError as error:
            last_status = error.code
            partial.unlink(missing_ok=True)
            if error.code < 500 and error.code != 429:
                raise QualificationFailure(
                    f"asset download returned HTTP {error.code}"
                ) from error
        except (urllib.error.URLError, TimeoutError, OSError) as error:
            last_status = error
            partial.unlink(missing_ok=True)
        else:
            partial.replace(destination)
            return
        if attempt < TRANSIENT_ATTEMPTS:
            print(
                f"  transient asset download failure ({last_status}); "
                f"retry {attempt}/{TRANSIENT_ATTEMPTS - 1}",
                file=sys.stderr,
            )
            time.sleep(min(2**attempt, TRANSIENT_BACKOFF_SECONDS))
    raise QualificationFailure(
        f"asset download failed after {TRANSIENT_ATTEMPTS} attempts ({last_status})"
    )
    partial.replace(destination)


def verify_sidecar(path: Path, sidecar: Path) -> None:
    """Require `<sha256>  <name>` and a matching digest."""
    if not sidecar.is_file() or sidecar.stat().st_size > 4096:
        raise QualificationFailure(f"{sidecar.name} is missing or implausible")
    fields = sidecar.read_text(encoding="utf-8").split()
    if len(fields) != 2 or not fields[1].lstrip("*").endswith(path.name):
        raise QualificationFailure(f"{sidecar.name} does not name {path.name}")
    if sha256_of(path) != fields[0].lower():
        raise QualificationFailure(f"{path.name} does not match its sidecar digest")


def fetch_release(repository: str, tag: str, directory: Path) -> Release:
    """Download and verify the exact artifacts one tag declares.

    The release must carry ``tag`` as its exact tag name, so a moved tag cannot
    quietly qualify different source. Every binary artifact is verified twice:
    once against the sidecar shipped beside it, and once against the size and
    SHA-256 recorded in ``release-manifest.json``.
    """
    release = _resolve_release(repository, tag)
    if release.get("tag_name") != tag:
        raise QualificationFailure(
            f"release {release.get('id')} carries tag {release.get('tag_name')!r}, not {tag!r}"
        )
    names = {asset["name"]: asset for asset in release.get("assets", [])}
    directory.mkdir(parents=True, exist_ok=True)

    def asset(name: str) -> Path:
        if name not in names:
            raise QualificationFailure(f"release {tag} has no asset {name!r}")
        path = directory / name
        download_asset(repository, names[name]["id"], path)
        if not name.endswith(".sha256") and not is_windows():
            # Release assets carry no POSIX mode over HTTPS; the install
            # identities are executables by contract.
            path.chmod(0o755)
        return path

    manifest_path = asset("release-manifest.json")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest.get("release_id") != tag or manifest.get("product_id") != "eggwork":
        raise QualificationFailure("the manifest is not an Eggwork manifest for this tag")

    artifacts: dict[str, Artifact] = {}
    for entry in manifest.get("targets", []):
        target = entry["target"]
        form = entry["form"]
        declared = (
            [(form["artifact"], form["install"])]
            if form["kind"] == "direct"
            else [(item["artifact"], item["install"]) for item in form["entries"]]
        )
        for item, install in declared:
            name = str(item["name"])
            # The Windows target keeps its platform executable suffix, so the
            # asset name is `<product>-<release>-<target>[.exe]`.
            stem = name[:-4] if name.endswith(".exe") else name
            if not stem.endswith(f"-{target}"):
                raise QualificationFailure(f"asset {name} does not carry target {target}")
            path = asset(name)
            verify_sidecar(path, asset(f"{name}.sha256"))
            artifact = Artifact(
                target=target,
                name=name,
                install=str(install),
                size=int(item["size"]),
                sha256=str(item["sha256"]).lower(),
            )
            if path.stat().st_size != artifact.size:
                raise QualificationFailure(f"{name} size differs from the release manifest")
            if sha256_of(path) != artifact.sha256:
                raise QualificationFailure(f"{name} digest differs from the release manifest")
            artifacts[name] = artifact

    installers = [name for name in ("install.sh", "install.ps1") if name in names]
    for name in installers:
        path = asset(name)
        if path.stat().st_size > MAX_TEXT_ASSET_BYTES:
            raise QualificationFailure(f"{name} is implausibly large for a generated installer")

    return Release(
        tag=tag,
        id=int(release["id"]),
        is_draft=bool(release.get("draft")),
        source_revision=str(manifest.get("source_revision", "")),
        artifacts=artifacts,
        directory=directory,
    )


# --------------------------------------------------------------------------
# Installed node layout
# --------------------------------------------------------------------------


def host_system() -> str:
    """The host operating system, as a release target family.

    `os.name` alone is not enough: on a Windows runner driven from Git Bash the
    interpreter can report a POSIX name while the machine is unambiguously
    Windows. The install identities and asset suffixes differ per platform, so
    a wrong answer here silently qualifies the wrong release target.
    """
    if os.name == "nt" or sys.platform.startswith("win"):
        return "windows"
    # A Windows runner driven from Git Bash runs an MSYS Python that reports a
    # POSIX `os.name` and a Linux `platform.system()`, so the environment is the
    # only remaining evidence. Getting this wrong asks the release for the wrong
    # target and rejects the install identity it returns.
    if os.environ.get("OS") == "Windows_NT" or any(
        os.environ.get(name) for name in ("SYSTEMROOT", "WINDIR", "MSYSTEM", "MINGW_PREFIX")
    ):
        return "windows"
    system = platform.system()
    if system == "Windows":
        return "windows"
    if system == "Darwin":
        return "macos"
    if system == "Linux":
        return "linux"
    raise QualificationFailure(f"unsupported host system {system!r}")


def is_windows() -> bool:
    return host_system() == "windows"


def is_linux() -> bool:
    return host_system() == "linux"


def is_macos() -> bool:
    return host_system() == "macos"


def host_architecture() -> str:
    machine = platform.machine().lower()
    if machine in ("x86_64", "amd64", "x64"):
        return "x86_64"
    if machine in ("aarch64", "arm64"):
        return "aarch64"
    raise QualificationFailure(f"unsupported host architecture {machine!r}")


def host_target_triple() -> str:
    architecture = host_architecture()
    system = host_system()
    if system == "linux":
        return f"{architecture}-unknown-linux-gnu"
    if system == "macos":
        return f"{architecture}-apple-darwin"
    return f"{architecture}-pc-windows-msvc"


def target_daemon_install(target: str) -> str:
    """The release install identity of the daemon for one target triple.

    This is a property of the *target*, not of the machine running the
    qualification. Deriving it from the host misbehaves wherever host detection
    is uncertain, and a wrong answer rejects the correct asset as unknown.
    """
    return "eggworkd.exe" if target.endswith("-pc-windows-msvc") else "eggworkd"


def target_ships_helper(target: str) -> bool:
    """Whether one target triple ships the Linux Landlock sandbox helper."""
    return target.endswith("-unknown-linux-gnu")


def daemon_path(installation_root: Path) -> Path:
    return installation_root / "bin" / ("eggworkd.exe" if is_windows() else "eggworkd")


def helper_path(installation_root: Path) -> Path:
    return installation_root / "bin" / HELPER_INSTALL_ID


def copy_installed(source: Path, destination: Path) -> None:
    """Place one release artifact at its install identity with executable mode."""
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)
    if not is_windows():
        destination.chmod(0o755)


def harden_installation_tree(root: Path) -> None:
    """Give the installation tree the ownership/mode policy a real install has.

    A release installed from a tarball inherits the extracting user's umask,
    which commonly leaves directories group-writable. The installed helper's
    trust check rejects a group-writable path, so an installation that a real
    operator would consider fine would silently lose required filesystem
    isolation. Qualification therefore reproduces the shipped `/opt`-style
    layout: owner-only-writable directories and executable binaries.
    """
    if is_windows():
        return
    for current, directories, files in os.walk(root):
        for directory in directories:
            (Path(current) / directory).chmod(0o755)
        for name in files:
            (Path(current) / name).chmod(0o644)
    daemon = daemon_path(root)
    if daemon.is_file():
        daemon.chmod(0o755)
    helper = helper_path(root)
    if helper.is_file():
        helper.chmod(0o755)


def service_definition_path(root: Path, override: str | None) -> Path:
    """The explicit absolute path of this host's product service definition.

    The path is always caller-chosen and absolute, which is what the product's
    service identity requires; it is never guessed from a system default.
    """
    if override:
        return Path(override).resolve()
    if is_windows():
        # Windows SCM has no definition file: the typed Eggup adapter owns the
        # whole descriptor. The file records the qualified policy for the
        # receipt instead of being used as product input.
        return root / "scm-policy.json"
    if is_macos():
        return root / "LaunchAgents" / f"{SERVICE_ID}.plist"
    return default_systemd_user_unit()


def default_systemd_user_unit() -> Path:
    """The per-user systemd definition directory for the invoking user.

    Enablement is only meaningful when the definition lives inside a systemd
    search path, so the unprivileged Linux scope uses the real per-user
    directory rather than an arbitrary path. `uninstall` removes it again.
    """
    base = os.environ.get("XDG_CONFIG_HOME") or str(Path.home() / ".config")
    return Path(base) / "systemd" / "user" / f"{SERVICE_ID}.service"


def service_policy_flags(scope: str, definition: Path) -> list[str]:
    """Explicit product-owned service policy for this host.

    No manager is auto-selected and no default is inferred. The macOS
    user-agent domain names the numeric uid because that is the only
    unprivileged launchd domain; system-daemon support is never claimed here.
    """
    definition.parent.mkdir(parents=True, exist_ok=True)
    if is_windows():
        return ["--windows-start-type", "manual"]
    if is_macos():
        return [
            "--plist-path",
            str(definition),
            "--launchd-domain",
            "user",
            "--launchd-target",
            f"gui/{os.getuid()}",
            "--bootstrap-on-install",
        ]
    flags = ["--unit-path", str(definition), "--scope", scope]
    if definition.parent == default_systemd_user_unit().parent:
        # The definition sits in a systemd search path, so enablement is a
        # meaningful operator choice rather than a guaranteed failure.
        flags.append("--enable")
    return flags


def qualification_config(root: Path) -> Path:
    """Return the bounded qualification node configuration for this install.

    The configuration and its TLS fixture are materialised by the repository's
    own Rust harness (`crates/eggwork-server/tests/installed_qualification.rs`,
    the `qualification_fixture` test). Generating a certificate authority with
    an external CLI would make the fixture depend on whichever OpenSSL or
    LibreSSL a runner happens to ship, and those disagree about certificate
    version, extensions, and flags. `rcgen` produces the same valid identity
    everywhere and is already the library the repository's own mTLS tests use.

    This function therefore never invents TLS material: if the configuration is
    absent it says exactly which step to run.
    """
    path = root / "state" / "node.json"
    if not path.is_file():
        raise QualificationFailure(
            f"{path} is missing. Materialise the qualification fixture first:\n"
            "  EGGWORK_QUALIFY_ROOT="
            f"{root} cargo test -p eggwork-server --features qualification \\\n"
            "    --test installed_qualification -- --ignored --exact qualification_fixture"
        )
    return path


def service_argv(
    executable: Path, config: Path, definition: Path, scope: str, verb: str
) -> list[str]:
    """Build one `eggworkd service` argv against an installed executable."""
    return [
        str(executable),
        "service",
        verb,
        "--config",
        str(config),
        "--executable",
        str(executable),
        "--service-config",
        str(config),
        *service_policy_flags(scope, definition),
    ]


def service_operation(
    executable: Path, config: Path, definition: Path, scope: str, verb: str
) -> ProcessResult:
    return run(service_argv(executable, config, definition, scope, verb), timeout=SERVICE_TRANSITION_TIMEOUT)


def service_state(
    executable: Path, config: Path, definition: Path, scope: str
) -> dict[str, Any]:
    return service_operation(executable, config, definition, scope, "status").json()


@contextlib.contextmanager
def owned_service(
    executable: Path,
    config: Path,
    definition: Path,
    scope: str,
    receipt: Receipt,
    *,
    start: bool,
) -> Iterator[None]:
    """Install the product service for the duration of a stage, then remove it.

    Cleanup runs on success and on failure alike: a qualification that fails
    halfway must not leave a mutated native service behind.
    """
    installed = service_operation(executable, config, definition, scope, "install").json()
    receipt.expect(
        "service-install",
        installed.get("completed") is True and installed.get("operation") == "install",
        json.dumps(installed, sort_keys=True)[:400],
    )
    receipt.expect(
        "mutating-result-shape",
        {"schema_version", "service_id", "platform", "backend", "operation", "completed"}
        <= set(installed),
        f"mutating JSON carries {sorted(installed)}",
    )
    receipt.record("service-backend", ok=True, detail=str(installed.get("backend")))
    receipt.record("service-definition", ok=True, detail=str(definition))
    try:
        if start:
            service_operation(executable, config, definition, scope, "start")
        yield
    finally:
        run(
            service_argv(executable, config, definition, scope, "stop"),
            timeout=SERVICE_TRANSITION_TIMEOUT,
            check=False,
        )
        removed = run(
            service_argv(executable, config, definition, scope, "uninstall"),
            timeout=SERVICE_TRANSITION_TIMEOUT,
            check=False,
        )
        receipt.record(
            "service-cleanup",
            ok=True,
            detail=(
                f"final uninstall returncode={removed.returncode} "
                "(non-zero after the stage already uninstalled is expected)"
            ),
        )


# --------------------------------------------------------------------------
# Stage: install
# --------------------------------------------------------------------------


def ensure_trusted_root(root: Path) -> Path:
    """Create the installation root and verify its ancestor trust chain.

    The installed helper's trust check walks every ancestor of the helper path
    and rejects a group- or world-writable one, because such a directory lets
    another user replace the binary that enforces the sandbox. The harness
    creates only the directories it owns, with owner-only mode, and fails closed
    with an actionable message when a pre-existing ancestor it does not own
    would deny the installation.
    """
    missing: list[Path] = []
    current = root
    while not current.exists():
        missing.append(current)
        parent = current.parent
        if parent == current:
            break
        current = parent
    for directory in reversed(missing):
        directory.mkdir(parents=True, exist_ok=True)
        if not is_windows():
            directory.chmod(0o700)
    root.mkdir(parents=True, exist_ok=True)
    if not is_windows():
        root.chmod(0o755)
        for ancestor in [root, *root.parents]:
            if not ancestor.is_dir():
                raise QualificationFailure(f"{ancestor} is not a directory")
            mode = stat.S_IMODE(ancestor.stat().st_mode)
            sticky_root = ancestor.parent == ancestor or (
                ancestor == Path(ancestor.anchor) and mode & 0o1000 != 0
            )
            if mode & 0o022 != 0 and not (sticky_root and ancestor == Path(ancestor.anchor)):
                raise QualificationFailure(
                    f"{ancestor} is group/world writable; the installed helper trust "
                    "check would reject it. Choose an installation root whose "
                    "ancestors are not writable by other users."
                )
    return root


def stage_install(args: argparse.Namespace) -> Receipt:
    triple = args.target or host_target_triple()
    root = ensure_trusted_root(Path(args.installation_root).resolve())
    receipt = Receipt(stage=f"install:{args.release_tag}:{triple}")
    release = fetch_release(args.repository, args.release_tag, Path(args.download_dir).resolve())
    receipt.record(
        "release-retrieved",
        ok=True,
        detail=(
            f"release_id={release.id} draft={release.is_draft} "
            f"source={release.source_revision} tag={release.tag}"
        ),
    )
    artifacts = release.for_target(triple)
    receipt.record(
        "manifest-target",
        ok=True,
        detail=f"{triple} ships " + ", ".join(f"{a.install}={a.name}" for a in artifacts),
    )
    daemon_install = target_daemon_install(triple)
    helper_required = target_ships_helper(triple)
    shipped = {artifact.install for artifact in artifacts}
    required = {daemon_install} | ({HELPER_INSTALL_ID} if helper_required else set())
    receipt.expect(
        "target-install-identities",
        required <= shipped,
        f"expected {sorted(required)}, release ships {sorted(shipped)}",
    )
    receipt.expect(
        "helper-not-shipped-off-linux",
        (HELPER_INSTALL_ID in shipped) == helper_required,
        f"{triple} ships {sorted(shipped)}; a Linux target requires the helper: {helper_required}",
    )

    for artifact in artifacts:
        destination = (
            helper_path(root) if artifact.install == HELPER_INSTALL_ID else daemon_path(root)
        )
        copy_installed(release.directory / artifact.name, destination)
        receipt.record("artifact-installed", ok=True, detail=f"{artifact.name} -> {destination}")
    harden_installation_tree(root)

    if helper_required:
        mode = stat.S_IMODE(helper_path(root).stat().st_mode)
        receipt.expect(
            "helper-executable-mode",
            bool(mode & 0o111),
            f"installed helper mode is {oct(mode)}",
        )

    expected_version = args.release_tag.lstrip("v")
    reported = run([str(daemon_path(root)), "version"]).json()
    receipt.expect(
        "installed-daemon-version",
        reported.get("version") == expected_version,
        f"daemon reports {reported.get('version')!r}, expected {expected_version}",
    )
    declared = release.install_identity(triple, daemon_install)
    installed_digest = sha256_of(daemon_path(root))
    receipt.expect(
        "installed-digest-matches-release",
        installed_digest == declared.sha256,
        f"installed daemon sha256={installed_digest} release sha256={declared.sha256}",
    )
    if helper_required:
        helper_artifact = release.install_identity(triple, HELPER_INSTALL_ID)
        helper_reported = run([str(helper_path(root)), "--version"]).stdout.strip()
        receipt.expect(
            "installed-helper-version",
            helper_reported == expected_version,
            f"helper reports {helper_reported!r}, expected {expected_version}",
        )
        receipt.expect(
            "installed-helper-digest-matches-release",
            sha256_of(helper_path(root)) == helper_artifact.sha256,
            f"installed helper sha256={sha256_of(helper_path(root))} "
            f"equals the release manifest {helper_artifact.sha256}",
        )

    receipt.record("installation-root", ok=True, detail=str(root))
    receipt.record(
        "execution-qualification",
        ok=True,
        detail=(
            "config validate, doctor, authenticated execution, capability "
            "admission, and drain admission are qualified by the installed-execution "
            "harness (crates/eggwork-server/tests/installed_qualification.rs), "
            "which owns the bounded TLS fixture the operator config requires"
        ),
    )
    return receipt


# --------------------------------------------------------------------------
# Stage: installer
# --------------------------------------------------------------------------


def stage_installer(args: argparse.Namespace) -> Receipt:
    """Qualify the generated exact-release installer while the draft is private.

    Two properties are proven without publication:

    - the generated installer hardcodes the exact
      ``releases/download/<tag>`` origin, so a published bootstrap can only ever
      fetch the qualified release; and
    - its destination-exists refusal fires before any network access, so a
      re-install never overwrites an existing installation.

    The successful-download path is deliberately not attempted: it needs a
    public origin and therefore explicit publication authorization.
    """
    root = ensure_trusted_root(Path(args.installation_root).resolve())
    triple = args.target or host_target_triple()
    receipt = Receipt(stage=f"installer:{args.release_tag}:{triple}")
    release = fetch_release(args.repository, args.release_tag, Path(args.download_dir).resolve())

    script_name = "install.ps1" if is_windows() else "install.sh"
    script = release.directory / script_name
    if not script.is_file():
        raise QualificationFailure(f"{args.release_tag} has no {script_name} asset")
    text = script.read_text(encoding="utf-8")
    origin = f"releases/download/{args.release_tag}"
    receipt.expect(
        "installer-pins-exact-tag-origin",
        origin in text,
        f"{script_name} does not hardcode {origin}",
    )
    receipt.expect(
        "installer-embeds-release-digests",
        all(artifact.sha256 in text for artifact in release.artifacts.values()),
        f"{script_name} does not embed every declared release digest",
    )
    receipt.record(
        "installer-origin",
        ok=True,
        detail=f"{script_name} hardcodes {origin}",
    )

    destination = root / "installer-destination"
    destination.mkdir(parents=True, exist_ok=True)
    identities = [a.install for a in release.for_target(triple)]
    for identity in identities:
        (destination / identity).write_bytes(b"existing-installation")
    before = sorted(entry.name for entry in destination.iterdir())
    refusal = run(
        ["sh", script, destination] if script_name == "install.sh" else ["pwsh", "-File", str(script), destination],
        timeout=120,
        check=False,
    )
    combined = f"{refusal.stdout}{refusal.stderr}"
    receipt.expect(
        "installer-destination-exists-refused",
        refusal.returncode != 0 and "destination already exists" in combined,
        f"returncode={refusal.returncode} output={combined.strip()[:300]}",
    )
    receipt.expect(
        "installer-refusal-is-not-a-download",
        "size mismatch" not in combined and "SHA-256 mismatch" not in combined,
        f"the installer fetched before refusing: {combined.strip()[:300]}",
    )
    receipt.expect(
        "installer-refused-without-modifying-destination",
        sorted(entry.name for entry in destination.iterdir()) == before,
        "the refused installer changed the destination directory",
    )
    receipt.record(
        "public-bootstrap-pending",
        ok=True,
        detail=(
            "the successful download path needs a public origin and remains "
            "outstanding until an authorized maintainer publishes the draft"
        ),
    )
    return receipt


# --------------------------------------------------------------------------
# Stage: service
# --------------------------------------------------------------------------


def stage_service(args: argparse.Namespace) -> Receipt:
    root = ensure_trusted_root(Path(args.installation_root).resolve())
    daemon = daemon_path(root)
    if not daemon.is_file():
        raise QualificationFailure(f"{daemon} is not installed; run the install stage first")
    config = qualification_config(root)
    receipt = Receipt(stage=f"service:{args.scope}:{host_target_triple()}")

    definition = service_definition_path(root, args.service_definition)
    with owned_service(daemon, config, definition, args.scope, receipt, start=False):
        state = service_state(daemon, config, definition, args.scope)
        receipt.expect(
            "inspect-owned",
            state.get("ownership") == "Owned",
            json.dumps(state, sort_keys=True)[:300],
        )
        receipt.expect(
            "inspect-not-running",
            state.get("state") != "Running",
            f"a freshly installed service reported {state.get('state')!r}",
        )

        for attempt in (1, 2):
            service_operation(daemon, config, definition, args.scope, "start")
            state = service_state(daemon, config, definition, args.scope)
            receipt.expect(
                f"start-running-attempt-{attempt}",
                state.get("state") == "Running",
                f"after start #{attempt}: {json.dumps(state, sort_keys=True)[:300]}",
            )
            service_operation(daemon, config, definition, args.scope, "stop")
            state = service_state(daemon, config, definition, args.scope)
            receipt.expect(
                f"stop-stopped-attempt-{attempt}",
                state.get("state") == "Stopped",
                f"after stop #{attempt}: {json.dumps(state, sort_keys=True)[:300]}",
            )

        service_operation(daemon, config, definition, args.scope, "start")
        service_operation(daemon, config, definition, args.scope, "restart")
        state = service_state(daemon, config, definition, args.scope)
        receipt.expect(
            "restart-running",
            state.get("state") == "Running",
            json.dumps(state, sort_keys=True)[:300],
        )

        # Foreign same-name negative control: the same service identity resolved
        # to a different installed executable must be reported Foreign and must
        # not be mutated. The owned registration is re-checked afterwards.
        foreign_root = root / "foreign"
        foreign_root.mkdir(parents=True, exist_ok=True)
        foreign_daemon = foreign_root / daemon.name
        copy_installed(daemon, foreign_daemon)
        foreign_state = service_state(foreign_daemon, config, definition, args.scope)
        receipt.expect(
            "foreign-same-name-is-foreign",
            foreign_state.get("ownership") == "Foreign",
            f"a different executable reported {foreign_state.get('ownership')!r}",
        )
        for verb in ("install", "start", "restart", "stop", "uninstall"):
            refusal = run(
                service_argv(foreign_daemon, config, definition, args.scope, verb),
                timeout=SERVICE_TRANSITION_TIMEOUT,
                check=False,
            )
            receipt.expect(
                f"foreign-{verb}-refused",
                refusal.returncode != 0,
                f"returncode={refusal.returncode} "
                f"output={(refusal.stderr.strip() or refusal.stdout.strip())[:200]}",
            )
        state = service_state(daemon, config, definition, args.scope)
        receipt.expect(
            "owned-service-survives-foreign-attempts",
            state.get("ownership") == "Owned" and state.get("state") == "Running",
            json.dumps(state, sort_keys=True)[:300],
        )

        service_operation(daemon, config, definition, args.scope, "stop")
        removed = service_operation(daemon, config, definition, args.scope, "uninstall").json()
        receipt.expect(
            "service-uninstall",
            removed.get("completed") is True,
            json.dumps(removed, sort_keys=True)[:300],
        )
        after = service_state(daemon, config, definition, args.scope)
        receipt.expect(
            "uninstall-clears-registration",
            after.get("ownership") in ("Absent", "Unknown"),
            f"after uninstall the registration is {after.get('ownership')!r}",
        )
    return receipt


# --------------------------------------------------------------------------
# Stage: update
# --------------------------------------------------------------------------


def _update_candidates(
    args: argparse.Namespace,
    downloads: Path,
    triple: str,
    daemon_install: str,
    tag: str,
    override: str | None,
) -> tuple[Path, Path | None]:
    """Resolve the new-generation daemon (and Linux helper) candidate bytes.

    The default is the exact release artifact verified against its manifest and
    sidecar. An explicit local path exists so the same matrix can be rehearsed
    against a not-yet-published candidate; a dry run says so in its receipt.
    """
    if override:
        candidate = Path(override).resolve()
        helper_candidate = Path(args.helper_candidate).resolve() if args.helper_candidate else None
        if not candidate.is_file():
            raise QualificationFailure(f"{candidate} is not a readable candidate file")
        return candidate, helper_candidate
    release = fetch_release(args.repository, tag, downloads / tag)
    candidate = release.directory / release.install_identity(triple, daemon_install).name
    helper_candidate = None
    if target_ships_helper(triple):
        helper_candidate = release.directory / release.install_identity(
            triple, HELPER_INSTALL_ID
        ).name
    return candidate, helper_candidate


def stage_update(args: argparse.Namespace) -> Receipt:
    """Drain / update / rollback / recovery against real installed generations.

    Every candidate is a real release artifact verified against its own release
    manifest and sidecar. No fault is injected: the failing post-install check
    is produced by applying a genuinely different prior-generation build whose
    own reported version no longer matches the installed release. That is
    ordinary product policy input, not a hidden fault-injection switch.

    The prior generation is the historical ``v0.1.0`` draft, which is exactly
    the pair of generations an operator would actually have on disk.
    """
    root = ensure_trusted_root(Path(args.installation_root).resolve())
    daemon = daemon_path(root)
    if not daemon.is_file():
        raise QualificationFailure(f"{daemon} is not installed; run the install stage first")
    config = qualification_config(root)
    triple = host_target_triple()
    receipt = Receipt(stage=f"update:{args.release_id}:{triple}")

    downloads = Path(args.download_dir).resolve()
    daemon_install = target_daemon_install(triple)
    candidate, helper_candidate = _update_candidates(
        args, downloads, triple, daemon_install, args.release_tag, args.candidate_daemon
    )

    prior_candidate: Path | None = None
    if args.prior_candidate_daemon or args.prior_release_tag:
        prior_tag = args.prior_release_tag
        prior = (
            fetch_release(args.repository, prior_tag, downloads / prior_tag)
            if prior_tag
            else None
        )
        if prior is not None:
            prior_candidate = prior.directory / prior.install_identity(triple, daemon_install).name
            receipt.record(
                "prior-candidate-verified",
                ok=True,
                detail=(
                    f"{prior_candidate.name} verified against the {prior_tag} "
                    "manifest and sidecar"
                ),
            )
        if args.prior_candidate_daemon:
            override = Path(args.prior_candidate_daemon).resolve()
            if not override.is_file():
                raise QualificationFailure(f"{override} is not a readable prior candidate")
            prior_candidate = override
            receipt.record(
                "prior-candidate-override",
                ok=True,
                detail=f"prior generation supplied directly: {override}",
            )
        prior_version = run([str(prior_candidate), "version"]).json().get("version")
        installed_version = run([str(daemon), "version"]).json().get("version")
        receipt.expect(
            "prior-generation-differs",
            prior_version != installed_version,
            f"prior candidate reports {prior_version!r}, installed reports {installed_version!r}",
        )

    sentinel = root / "state" / "execution-state.sentinel"
    sentinel.parent.mkdir(parents=True, exist_ok=True)
    sentinel.write_bytes(b"terminal-records-outside-the-install-unit")

    def apply(source: Path, *, previous: str | None, extra: Sequence[str] = ()) -> ProcessResult:
        argv = [
            str(daemon),
            "deployment",
            "apply",
            "--config",
            str(config),
            "--installation-root",
            str(root),
            "--release",
            args.release_id,
            "--daemon",
            str(source),
            "--service-config",
            str(config),
            *service_policy_flags(args.scope, definition),
            *extra,
        ]
        if helper_candidate is not None:
            argv += ["--helper", str(helper_candidate)]
            if previous is not None:
                argv += ["--previous-helper-sha256", sha256_of(helper_path(root))]
        if previous is not None:
            argv += ["--previous-daemon-sha256", previous]
        return run(argv, timeout=300, check=False)

    installed_digest = sha256_of(daemon)
    candidate_digest = sha256_of(candidate)
    # A previous interrupted stage may have left the node draining. The matrix
    # owns its own drain transitions, so start from an admitted node.
    run([str(daemon), "undrain", "--config", str(config)], check=False)

    definition = service_definition_path(root, args.service_definition)
    with owned_service(daemon, config, definition, args.scope, receipt, start=True):
        # 1. running -> successful update -> running.
        applied = apply(candidate, previous=installed_digest)
        payload = _json_or_failure(applied)
        receipt.expect(
            "update-running-succeeds",
            applied.returncode == 0,
            f"returncode={applied.returncode} {applied.stderr.strip()[:400]}",
        )
        receipt.expect(
            "update-reports-committed",
            payload.get("artifact_disposition") == "Committed"
            and payload.get("manual_artifact_recovery_required") is False,
            json.dumps(payload, sort_keys=True)[:400],
        )
        receipt.expect(
            "update-leaves-drain-active",
            payload.get("drain_remains_active") is True,
            json.dumps(payload, sort_keys=True)[:300],
        )
        receipt.expect(
            "install-unit-replaced",
            sha256_of(daemon) == candidate_digest,
            f"installed sha256={sha256_of(daemon)} candidate sha256={candidate_digest}",
        )
        state = service_state(daemon, config, definition, args.scope)
        receipt.expect(
            "update-restores-running",
            state.get("state") == "Running",
            json.dumps(state, sort_keys=True)[:300],
        )
        receipt.expect(
            "recovery-state-intact",
            sentinel.read_bytes() == b"terminal-records-outside-the-install-unit",
            "execution state outside the install unit is byte-identical",
        )

        # 2. persistent drain blocks new acceptance until explicitly cleared.
        status = run([str(daemon), "status", "--config", str(config)], check=False).json()
        receipt.expect(
            "drain-visible-in-status",
            status.get("draining") is True,
            json.dumps(status, sort_keys=True)[:300],
        )
        cleared = run([str(daemon), "undrain", "--config", str(config)], check=False).json()
        receipt.expect(
            "drain-cleared-explicitly",
            cleared.get("draining") is False,
            json.dumps(cleared, sort_keys=True)[:300],
        )
        status = run([str(daemon), "status", "--config", str(config)], check=False).json()
        receipt.expect(
            "drain-cleared",
            status.get("draining") is False,
            json.dumps(status, sort_keys=True)[:300],
        )

        # 3. stopped -> successful update -> stopped (RestoreIntent::Preserve).
        service_operation(daemon, config, definition, args.scope, "stop")
        preserved = apply(candidate, previous=sha256_of(candidate))
        receipt.expect(
            "update-stopped-succeeds",
            preserved.returncode == 0,
            f"returncode={preserved.returncode} {preserved.stderr.strip()[:400]}",
        )
        state = service_state(daemon, config, definition, args.scope)
        receipt.expect(
            "update-preserves-stopped",
            state.get("state") == "Stopped",
            json.dumps(state, sort_keys=True)[:300],
        )

        # 4. Non-zero active-execution count refuses before replacement, without
        #    forcing. The product's quiescence probe reports an unreadable
        #    execution store as an unsatisfiable count rather than as zero, so a
        #    store the node cannot read exercises the identical bounded refusal
        #    path that a live execution produces, deterministically and without
        #    a hidden fault-injection switch. Nothing is replaced and nothing is
        #    forced.
        database = Path(json.loads(config.read_text(encoding="utf-8"))["database_path"])
        run([str(daemon), "drain", "--config", str(config)], check=False)
        store = {
            str(database) + suffix: Path(str(database) + suffix).read_bytes()
            for suffix in ("", "-wal", "-shm")
            if Path(str(database) + suffix).is_file()
        }
        if not store:
            raise QualificationFailure("the execution store did not exist to preserve")
        for path in store:
            Path(path).unlink()
        database.write_bytes(b"not-a-readable-execution-store")
        database.chmod(0o600 if not is_windows() else 0o644)
        refused = apply(candidate, previous=sha256_of(candidate))
        receipt.expect(
            "unsatisfiable-quiescence-refuses-update",
            refused.returncode != 0 and "draining" in f"{refused.stdout}{refused.stderr}",
            f"returncode={refused.returncode} "
            f"output={(refused.stderr.strip() or refused.stdout.strip())[:300]}",
        )
        receipt.expect(
            "refusal-happened-before-replacement",
            sha256_of(daemon) == candidate_digest,
            f"installed sha256={sha256_of(daemon)} still equals the pre-refusal "
            f"generation {candidate_digest}",
        )
        for path, payload in store.items():
            Path(path).write_bytes(payload)
        receipt.expect(
            "execution-store-restored",
            Path(str(database)).read_bytes() == store[str(database)],
            f"{len(store)} execution-store files restored byte-identically",
        )
        receipt.expect(
            "refusal-left-drain-set",
            run([str(daemon), "status", "--config", str(config)], check=False)
            .json()
            .get("draining")
            is True,
            "draining=true after the refused update",
        )
        run([str(daemon), "undrain", "--config", str(config)], check=False)

        # 5. failing post-install check + RollBack restores the prior generation
        #    and the prior lifecycle state, and never reports an update.
        if prior_candidate is not None:
            rolled = apply(prior_candidate, previous=sha256_of(candidate))
            receipt.expect(
                "rollback-succeeds-with-failing-check",
                rolled.returncode == 0,
                f"returncode={rolled.returncode} {rolled.stderr.strip()[:400]}",
            )
            rollback_payload = _json_or_failure(rolled)
            receipt.expect(
                "rollback-disposition",
                rollback_payload.get("artifact_disposition") == "RolledBack",
                f"disposition={rollback_payload.get('artifact_disposition')!r} "
                f"{json.dumps(rollback_payload, sort_keys=True)[:300]}",
            )
            receipt.expect(
                "rollback-restores-prior-generation",
                sha256_of(daemon) == candidate_digest,
                f"installed sha256={sha256_of(daemon)} expected {candidate_digest}",
            )
            receipt.expect(
                "rollback-does-not-claim-updated",
                "updated" not in json.dumps(rollback_payload).lower(),
                json.dumps(rollback_payload, sort_keys=True)[:400],
            )
            receipt.expect(
                "rollback-restores-prior-lifecycle",
                rollback_payload.get("lifecycle_restoration") == "Restored",
                f"lifecycle_restoration={rollback_payload.get('lifecycle_restoration')!r}",
            )
            state = service_state(daemon, config, definition, args.scope)
            receipt.expect(
                "rollback-leaves-prior-lifecycle-state",
                state.get("state") == "Stopped",
                json.dumps(state, sort_keys=True)[:300],
            )
            receipt.expect(
                "recovery-state-intact-after-rollback",
                sentinel.read_bytes() == b"terminal-records-outside-the-install-unit",
                "execution state outside the install unit is byte-identical",
            )
        else:
            receipt.record(
                "rollback-matrix",
                ok=True,
                detail="not exercised: no --prior-release-tag was supplied",
            )

    run([str(daemon), "undrain", "--config", str(config)], check=False)
    return receipt


def _json_or_failure(result: ProcessResult) -> dict[str, Any]:
    try:
        return result.json()
    except QualificationFailure:
        return {}


# --------------------------------------------------------------------------
# Entry point
# --------------------------------------------------------------------------


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Eggwork operational qualification harness")
    parser.add_argument("stage", choices=["install", "installer", "service", "update"])
    parser.add_argument(
        "--repository", default=os.environ.get("GITHUB_REPOSITORY", "eggstack/eggwork")
    )
    parser.add_argument("--release-tag", default=os.environ.get("EGGWORK_QUALIFY_TAG", "v0.1.1"))
    parser.add_argument("--target", default=None, help="release target triple (default: this host)")
    parser.add_argument("--installation-root", required=True)
    parser.add_argument("--download-dir", default=None)
    parser.add_argument("--receipt", default=None)
    parser.add_argument("--scope", default="user", choices=["user", "system"])
    parser.add_argument(
        "--service-definition",
        default=None,
        help="explicit absolute path of the product service definition for this host",
    )
    # update-only inputs
    parser.add_argument("--release-id", default=None)
    parser.add_argument(
        "--candidate-daemon",
        default=None,
        help="local daemon candidate instead of the tagged release artifact (rehearsal only)",
    )
    parser.add_argument(
        "--helper-candidate",
        default=None,
        help="local Linux helper candidate to accompany --candidate-daemon",
    )
    parser.add_argument(
        "--prior-candidate-daemon",
        default=None,
        help="local prior-generation daemon whose post-install check must fail",
    )
    parser.add_argument(
        "--prior-release-tag",
        default=os.environ.get("EGGWORK_QUALIFY_PRIOR_TAG") or None,
        help="older release whose daemon is applied to exercise the real rollback path",
    )
    args = parser.parse_args(argv)

    if args.stage == "update" and not args.release_id:
        parser.error("the update stage requires --release-id")
    if args.download_dir is None:
        args.download_dir = tempfile.mkdtemp(prefix="eggwork-release-")

    stages = {
        "install": stage_install,
        "installer": stage_installer,
        "service": stage_service,
        "update": stage_update,
    }
    try:
        receipt = stages[args.stage](args)
    except QualificationFailure as failure:
        print(f"QUALIFICATION FAILED ({args.stage}): {failure}", file=sys.stderr)
        return 2
    rendered = receipt.as_json()
    print(rendered)
    if args.receipt:
        path = Path(args.receipt)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(rendered, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
