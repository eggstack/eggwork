# Deployment and service lifecycle

Installing and updating a node as service software. Eggwork owns the *policy*;
[Eggup](https://crates.io/crates/eggup-service) owns the *mechanics*.

**Eggwork never manages its own service by shelling out.** The node service
lifecycle goes through Eggup adapters, which prove ownership before any
mutation. (The one deliberate exception: `eggwork-runner` invokes
`systemd-run --scope` and `systemctl show`/`stop` for *per-execution transient
cgroup units*. That is resource enforcement owned by the process-lifecycle
crate, not service lifecycle.)

## Service identity

Every `service` verb needs the exact executable and the exact registered config
path:

```bash
eggworkd service spec --config <node.json> \
  --executable /opt/eggwork/bin/eggworkd \
  --service-config /etc/eggwork/node.json
```

```json
{
  "schema_version": 1,
  "service_id": "eggwork-node",
  "executable": "/opt/eggwork/bin/eggworkd",
  "args": ["run", "--config", "/etc/eggwork/node.json"],
  "config": "/etc/eggwork/node.json"
}
```

`--service-config` is **required** and fails closed rather than defaulting: the
service identity must name the exact registered config path, so it is never
guessed from the operator config you happened to pass. `--executable` defaults
to the running binary's canonicalized path, but pass it explicitly in scripts.

The registered argv names the platform's service host: `service-host --config
<path>` on Windows, where the daemon joins the service control manager, and
`run` everywhere else. The systemd unit and the launchd plist render `run`
themselves, so they are unchanged.

`spec` is read-only and is the safe way to see what would be installed.

## Platform policy is explicit

A service backend is only claimed as supported where the native manager was
actually mutated on a real host. Adapter availability alone is never a support
claim. Each platform therefore requires its own policy flags:

| Platform | Required flags | Manager |
|---|---|---|
| Linux | `--unit-path <abs>` `--scope system\|user` | Eggup systemd adapter |
| macOS | `--plist-path <abs>` `--launchd-domain user\|system` `--launchd-target` | Eggup launchd adapter |
| Windows | none required; optional `--windows-start-type automatic\|manual\|disabled` (default `manual`) | Eggup Windows SCM adapter |

```bash
eggworkd service install --config <node.json> \
  --executable /opt/eggwork/bin/eggworkd --service-config /etc/eggwork/node.json \
  --unit-path /etc/systemd/system/eggwork-node.service --scope system

eggworkd service status  --config <node.json> \
  --executable /opt/eggwork/bin/eggworkd --service-config /etc/eggwork/node.json \
  --unit-path /etc/systemd/system/eggwork-node.service --scope system
```

`start`, `stop`, `restart`, and `uninstall` take the same flags. On macOS a
`user` domain additionally requires `--launchd-target gui/<uid>`.

Ownership is proven by **exact executable plus critical argv/config** before any
stop, replace, or uninstall. That is what stops a command from tearing down an
unrelated service that happens to share a name.

## Windows: the daemon hosts the service

**What the evidence said, and when.** Hosted qualification of an earlier release
candidate registered the `eggwork-node` SCM service and then failed to start it with
Windows error 1053. The cause was confirmed by inspection, not inferred: that
candidate's `eggworkd` implements **no service-control dispatcher**, so the
process the SCM launches never connects back to the manager and the service can
never reach `Running`. Registering an external process anyway would have left a
working installation with *no service behind it*, which is worse than an honest
refusal — so every mutating `service` verb on Windows was refused **before any
mutation**, with a message naming the unsupported backend.

**What is true now.** The daemon *is* a Windows service host.
`eggworkd service-host --config <absolute-path>` is the mode the service
control manager launches, and it is chosen explicitly from argv: the mode is
never inferred from the parent process, the session, the environment, or a
dispatcher error, and `run` keeps its existing meaning. On every other
platform the mode is refused outright.

The host reports its own transitions: `START_PENDING` before any startup work,
`RUNNING` only once the node is ready to serve, `STOP_PENDING` while the
active executions converge, and `STOPPED` at the end. A failed start and a
failed convergence each carry their own service exit code, and a bounded
checkpoint advances while a transition is pending, so a slow start does not
read as a hung service. `STOP` and `SHUTDOWN` are the same request and are
idempotent, and a stop that arrives during startup is latched rather than
lost — the node converges without ever having claimed to be running.

The `service` verbs are therefore live on Windows and go through Eggup's typed
SCM adapter, with the same ownership proof as every other platform: a
registration with a different executable, a different config, or a different
critical argv is **foreign**, and Eggwork never touches it.
`--windows-start-type` is a bounded knob — `automatic`, `manual`, or
`disabled`, defaulting to `manual` so an installed service does not start on
boot behind the operator's back.

A routine service stop is an **in-memory** drain: admission stops and the
active executions are cancelled and waited for, but no persistent drain marker
is written, an existing marker is preserved and still honoured at startup, and
service mode never undrains. The persistent marker stays the operator's
explicit mechanism (`eggworkd drain` / `eggworkd undrain`).

**What is not claimed.** The proof is a source-built hosted lifecycle: the
daemon is built from the repository and then registered, started, stopped,
restarted, and uninstalled against a real service control manager. The
*installed release* service lifecycle — release lineage and the exact installed
bytes — is a separate subject, and no published release carries the service
host yet, so there is no exact-release Windows service receipt.

## `deployment apply`

`deployment apply` updates an existing installation in place:

```bash
eggworkd deployment apply --config <node.json> \
  --installation-root /opt/eggwork \
  --release v0.1.5 \
  --daemon /var/tmp/eggworkd --helper /var/tmp/eggwork-sandbox-helper \
  --service-config /etc/eggwork/node.json \
  --unit-path /etc/systemd/system/eggwork-node.service --scope system
```

What it does and deliberately does not do:

- It accepts **already-local candidates only**. It does not fetch a URL,
  resolve a release, or publish.
- On Linux a release update requires **both** daemon and helper. macOS and
  Windows require the daemon only.
- A replacement can pass `--previous-daemon-sha256` and, on Linux,
  `--previous-helper-sha256`, so Eggup verifies the currently installed
  generation by exact digest. **Missing digest proof fails closed** for existing
  files.
- It leaves **persistent drain set after every outcome, including success**, for
  an explicit later `eggworkd undrain`. This is deliberate: an update that
  half-lands leaves the node refusing work until a human looks at it.

### Updates drain first

1. `eggworkd drain` — persistent, observed by the running node.
2. Wait for active executions to finish (or pass an explicit force policy
   through the orchestration API).
3. Replace.
4. Inspect the receipt, then `eggworkd undrain`.

### The receipt is the record

`deployment apply`'s JSON output is the operator's only record of what happened.
It reports the release id, the Core artifact disposition, whether manual artifact
recovery is required, the lifecycle restoration status, the final observed
ownership/state when the adapter can prove them, and the bounded failure detail
that explains a rolled-back or retained generation.

A `RecoveryRequired` disposition **never prints a successful update** — it names
the manual recovery instead. `RecoveryRequired` preserves Eggup's exact
transaction evidence and suppresses automatic restoration of the old service
generation; `RollBack` is the default and leaves rollback evidence available.

The post-install check reads the installed daemon's own `version` report and
requires an exact match with this release, then on Linux runs the installed
helper's trust and version check. A rolled-back disposition is therefore a real
post-install failure, not a guess.

## Design reference

[architecture/deployment-lifecycle.md](../architecture/deployment-lifecycle.md) ·
[architecture/distribution.md](../architecture/distribution.md) ·
[architecture/operations-cli.md](../architecture/operations-cli.md)
