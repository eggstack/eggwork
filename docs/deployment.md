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

`spec` is read-only and is the safe way to see what would be installed.

## Platform policy is explicit

A service backend is only claimed as supported where the native manager was
actually mutated on a real host. Adapter availability alone is never a support
claim. Each platform therefore requires its own policy flags:

| Platform | Required flags | Manager |
|---|---|---|
| Linux | `--unit-path <abs>` `--scope system\|user` | Eggup systemd adapter |
| macOS | `--plist-path <abs>` `--launchd-domain user\|system` `--launchd-target` | Eggup launchd adapter |
| Windows | *refused* | Typed adapter retained, unreachable |

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

## Windows: unsupported, and refused on purpose

Hosted qualification registered the `eggwork-node` SCM service and then failed to
start it with Windows error 1053. The cause was confirmed by inspection, not
inferred: `eggworkd` implements **no service-control dispatcher**, so the
process the SCM launches never connects back to the manager and the service can
never reach `Running`.

Registering an external process anyway would leave a working installation with
*no service behind it*, which is worse than an honest refusal. So every mutating
`service` verb on Windows is refused **before any mutation**, with a message
naming the unsupported backend. The typed adapter and the recorded SCM product
policy are retained in `deployment::windows_scm_manager` for the future
service-host milestone (Operations M007, status **ready** in
[`plans/registry.md`](../plans/registry.md)).

`--windows-start-type` appears in the qualification harness, but no code parses
it yet. Do not treat it as a supported knob.

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
