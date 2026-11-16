# Node operations

`eggworkd` is the operator CLI. Commands print JSON to stdout; errors go to
stderr and exit with status `2`. Commands that operate on a node take
`--config <file>`. The `version` command does not.

See the [quickstart](quickstart.md) for a working config. For installing a
service or updating binaries, see [deployment](deployment.md). When running
from a source checkout, use `./target/debug/eggworkd` in place of `eggworkd`.

## Commands

```text
eggworkd version
eggworkd config validate --config <node.json>
eggworkd config print    --config <node.json>
eggworkd doctor          --config <node.json>
eggworkd run             --config <node.json>
eggworkd status          --config <node.json>
eggworkd metrics         --config <node.json>
eggworkd drain           --config <node.json>
eggworkd undrain         --config <node.json>
eggworkd executions list --config <node.json> --limit 50 --offset 0
eggworkd executions show --config <node.json> <execution-id>
eggworkd storage summary --config <node.json>
eggworkd inspect blob <id>      --config <node.json>
eggworkd inspect workspace <id> --config <node.json>
eggworkd inspect artifact <id>  --config <node.json>
eggworkd gc              --config <node.json> --limit 128
eggworkd gc              --config <node.json> --apply --limit 128
eggworkd deployment status --config <node.json>
eggworkd deployment apply  --config <node.json> ...
eggworkd service spec|status|install|start|stop|restart|uninstall --config <node.json> ...
```

`gc` previews removals by default. Add `--apply` to delete eligible data.
Service and deployment options vary by platform; see [deployment](deployment.md).

## Readiness

Run `doctor` before starting the node. `ready` is true only when all eight
checks pass:

| Check | What it checks |
|---|---|
| `configuration` | Config and TLS material are valid. |
| `host-telemetry` | Optional telemetry provider config; disabled by default. |
| `sandbox-resource-backend` | Trusted helper and runtime-supported isolation and resource controls. |
| `listener` | The bind address is available. |
| `execution-root`, `blob-root`, `workspace-root` | Required directories are available and trusted. |
| `storage-headroom` | Free space covers configured storage quotas. |

`config validate` performs the same checks and reports `valid: true` when the
configuration is ready. It is useful in deployment scripts.

The helper and its path must pass an ownership and permission check. If that
check fails, the node refuses any required isolation guarantee. `doctor`
reports what the current host can enforce; it does not infer support from the
operating system alone. The [quickstart](quickstart.md#3-install-the-helper-where-it-is-trusted)
shows a trusted install location.

Start the node in the foreground with `run`. Stop it with Ctrl-C.

## Host telemetry

Host telemetry is optional and disabled by default. Without a `host_telemetry`
config block, the node reports no host measurements. To enable it, configure a
local provider:

```json
"host_telemetry": {
  "endpoint": "http://127.0.0.1:18080/v2/status",
  "refresh_interval_seconds": 15,
  "freshness_seconds": 60
}
```

The endpoint must use plain HTTP, an explicit port, a loopback IP address, and
the exact path `/v2/status`. Hostnames, redirects, proxies, credentials, query
strings, and fragments are not accepted. `freshness_seconds` must be at least
as long as `refresh_interval_seconds`.

An authorized controller reads cached measurements from `GET
/v1/node/telemetry`. Its client grant must include the separate `telemetry`
operation; `status` does not grant telemetry access.

| State | Meaning |
|---|---|
| `disabled` | No provider is configured. |
| `warming` | The provider has not returned a sample yet. |
| `ready` | A current sample is available. |
| `stale` | The last sample is older than its freshness limit. |
| `unavailable` | The provider could not be reached. |
| `invalid` | The provider response could not be used. |

Only `ready` means the sample is current. `unavailable` does not mean zero
utilization.

## Drain

```bash
eggworkd drain   --config <node.json>
eggworkd undrain --config <node.json>
```

Drain stops the node from accepting new executions and lets current executions
finish. The drain marker persists across restarts. Run `undrain` when the node
is ready to accept work again.

## Garbage collection

`gc` is a read-only preview unless `--apply` is specified:

```bash
eggworkd gc --config <node.json> --limit 128
```

Review the preview before applying it. Apply is bounded by `--limit`, takes an
exclusive lock on node state, and requires the daemon to be stopped. It first
expires old blob references, so it may remove more than the preview listed.
If the data roots have not been initialized, the preview reports an error
instead of claiming there is nothing to collect.

## Inspecting executions and data

```bash
eggworkd status --config <node.json>
eggworkd executions list --config <node.json> --limit 50 --offset 0
eggworkd executions show --config <node.json> <execution-id>
eggworkd inspect artifact <artifact-id> --config <node.json>
eggworkd storage summary --config <node.json>
eggworkd metrics --config <node.json>
```

`executions list` is paginated. Use its `next_offset` as the next `--offset`.
`metrics` includes each counter with a value of zero before it has been
incremented.

The `status` and execution list counts describe different things:

| Field | Meaning |
|---|---|
| `execution_permits_held` | Execution permits held by a running node. It is `null` when the node is stopped. |
| `non_terminal_executions` | Durable execution records that have not reached a terminal state. |

These counts can differ after a process failure or while the node is
recovering. Check both when investigating stuck work.

For executions with declared outputs, `executions show` reports
`artifact_capture`:

| Value | Meaning |
|---|---|
| `not_requested` | No outputs were declared. |
| `completed` | Outputs were captured and published. |
| `failed` | Outputs were not captured, and the execution did not succeed. |
| `unknown` | The node recovered the execution without a recorded capture result. |

## Configuration and redaction

The config loader rejects unknown fields, symlinks, unsafe ownership, and
unsafe permissions. Config and TLS files must be owned by root or the
invoking user and must not be group-writable or world-writable. The private
key must not be readable by other users. The execution, blob, and workspace
roots and the database parent must be owner-only (`0700`).

Configuration errors include a stable rule identifier, the affected file or
item, and a corrective action. For a private key or client certificate, the
message names its role without revealing its path, fingerprint, or subject.

`config print` redacts the private key path, client certificate fingerprints,
and resource-scope identifiers. Review the output before sharing it.

For service commands and update behavior, see [deployment](deployment.md).
