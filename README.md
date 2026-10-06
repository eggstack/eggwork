# eggwork

A Rust-native, fixed-target remote execution fabric. A caller selects one node,
submits a bounded execution request, observes machine-readable lifecycle events,
and receives terminal state plus declared artifacts.

Eggwork is deliberately **not another scheduler**. Global queues, worker
selection, priorities/fairness, workflow DAGs, and semantic retry stay
caller-owned, which is what lets it serve as an execution backend for an
orchestrator like CodeGG without competing with its policy.

The protocol-neutral core, the local process runner, the authenticated remote
control plane, durable leases, workspace transfer, declared artifact capture,
bounded retention/GC, Linux sandbox and resource controls, the node operations
surface, and Eggup-backed deployment are implemented. Downstream CodeGG
fixed-target integration through content-aware derived workspace transfer is
closed; the whole-AgentRun remote worker remains deferred.

## Quickstart

Build a node, give it real TLS material, and prove it executes a sandboxed
request. Full detail and troubleshooting: **[docs/quickstart.md](docs/quickstart.md)**.

```bash
# 1. Build both Linux binaries
cargo build --locked -p eggwork-server --bin eggworkd
cargo build --locked -p eggwork-sandbox-helper

./target/debug/eggworkd version
# {"version": "0.1.5"}
```

A node requires mTLS in both directions, so it will not start without real
certificate material. The quickstart mints a CA, a node server identity, and a
controller identity with `openssl`, and shows how Eggwork identifies a
controller: the SHA-256 of its certificate DER, lowercase hex.

```bash
# 2. Install the helper where it is trusted
install -m 0755 target/debug/eggwork-sandbox-helper \
  ~/.local/lib/eggwork/eggwork-sandbox-helper

# 3. Write a config (see examples/eggworkd.example.json for the field shape)
# 4. Set permissions — directories 0700, private key 0600, nothing group-writable
# 5. Prove it is ready
./target/debug/eggworkd doctor --config ./node.json
```

```json
{
  "ready": true,
  "checks": [
    { "name": "configuration", "ok": true, "detail": "configuration and TLS policy are valid" },
    { "name": "sandbox-resource-backend", "ok": true,
      "detail": "runtime-probed capabilities: isolation.landlock.workspace-rw.v1, resources.cgroups-v2.memory, resources.cgroups-v2.cpu, resources.cgroups-v2.pids" },
    { "name": "listener", "ok": true, "detail": "bind address is available" }
  ]
}
```

The `sandbox-resource-backend` line is a **runtime probe**, not a static claim:
it reports what this host can actually enforce right now.

```bash
# 6. Run the node
./target/debug/eggworkd run --config ./node.json
# {"local_addr":"127.0.0.1:7443","node_id":"worker-01","ready":true}
```

Operating it is JSON-only, so it composes with `jq` and never needs prose
parsing. Below, `eggworkd` is the installed binary; substitute
`./target/debug/eggworkd` when running from a source checkout:

```bash
eggworkd status          --config ./node.json
eggworkd drain           --config ./node.json   # persistent; refuse new work
eggworkd executions list --config ./node.json --limit 50
eggworkd storage summary --config ./node.json
eggworkd gc              --config ./node.json --limit 128   # read-only preview
```

Two properties worth knowing up front:

- **Isolation is earned, never assumed.** The node advertises filesystem
  isolation exactly when the installed helper passes its trust check, and the
  same function backs `doctor`, capability advertisement, and admission. A
  `Required` guarantee that cannot be enforced fails the execution; only an
  explicitly optional requirement degrades, and it records `NotApplied` with a
  reason.
- **Drain persists across restarts.** It is what makes it safe to drain, replace
  binaries, and investigate without racing the node back up. Clear it explicitly
  with `undrain`.

## Documentation

| | |
|---|---|
| [docs/quickstart.md](docs/quickstart.md) | Build a node, real TLS material, prove a confined execution |
| [docs/operations.md](docs/operations.md) | The `eggworkd` verb reference: readiness, drain, GC, inspection |
| [docs/deployment.md](docs/deployment.md) | Install and update a node as service software |
| [docs/releases.md](docs/releases.md) | Consume a published release; release identity and Windows reproducibility |

## Installing a published release

```bash
curl -fsSLO https://github.com/eggstack/eggwork/releases/download/vX.Y.Z/install.sh
less install.sh   # read it before running it
sh install.sh /opt/eggwork/bin
sha256sum -c eggwork-vX.Y.Z-<target>.sha256
```

The bootstrap scripts are **first-install only** — they never overwrite, never
register a service, never elevate privileges, and never select a release. Service
setup and updates are explicit operator policy through `eggworkd service …` and
Eggup.

Published targets: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`,
`x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`. Linux
ships the daemon and its Landlock helper as one bundle from one source revision;
macOS and Windows ship the daemon only.

SHA-256 sidecars are integrity evidence, not authenticity.

**Windows executes; it does not isolate.** A Windows node runs executions and
owns the whole process tree with a Job Object, so timeout, cancellation, output
limit, and a target that outlives its leader all converge. It offers no
filesystem or resource isolation: a `Required` isolation or resource request is
refused before the target starts, and a best-effort request is reported as
`NotApplied` rather than enforced.

**Windows service management is unsupported and fails closed** — `eggworkd` has
no service-control dispatcher, so a registered service can never reach
`Running`. Every mutating `service` verb is refused before any mutation; Windows
binary installation and daemon runtime are unaffected.

## Design and contribution

- [architecture/overview.md](architecture/overview.md) — module map, request flow, and the **divergences table** of known open questions
- [CONTRIBUTING.md](CONTRIBUTING.md) — the pre-submit gate
- [plans/registry.md](plans/registry.md) — current ready/blocked work
- [SECURITY.md](SECURITY.md) — report vulnerabilities privately
- [.skills/](.skills/README.md) — task-shaped agent guidance