# Node operations

`eggworkd` is the only supported way to operate a node. It is JSON-only: every
verb writes JSON to stdout and nothing else, so it composes with `jq` and never
requires parsing prose. Errors go to stderr and exit `2`.

Every verb takes `--config <file>` pointing at a versioned JSON config. See
[quickstart.md](quickstart.md) for how to create one that loads.

## Verbs

```text
eggworkd version
eggworkd config validate --config <node.json>
eggworkd config print    --config <node.json>
eggworkd doctor          --config <node.json>
eggworkd run             --config <node.json>
eggworkd status          --config <node.json>
eggworkd drain           --config <node.json>
eggworkd undrain         --config <node.json>
eggworkd executions list --config <node.json> --limit 50 --offset 0
eggworkd executions show --config <node.json> <execution-id>
eggworkd storage summary --config <node.json>
eggworkd inspect blob|workspace|artifact <id> --config <node.json>
eggworkd gc              --config <node.json> --limit 128         # read-only preview
eggworkd gc              --config <node.json> --apply --limit 128
eggworkd deployment status --config <node.json>
eggworkd deployment apply  --config <node.json> ...              # see deployment.md
eggworkd service spec|status|install|start|stop|restart|uninstall --config <node.json> ...
```

`eggworkd` with no arguments prints its usage and exits `2`.

## Readiness

`doctor` is the gate. It returns seven checks, and `ready` is true only when all
of them pass:

| Check | Meaning |
|---|---|
| `configuration` | Config parses and the TLS material loads into a usable server config |
| `sandbox-resource-backend` | The installed helper is trusted, and this host's actual isolation/resource capabilities were runtime-probed |
| `listener` | The bind address is available |
| `execution-root`, `blob-root`, `workspace-root` | Those directories exist and are trustworthy |
| `storage-headroom` | Free space is sufficient for the configured quotas |

`config validate` runs the same checks and additionally requires
`valid: true`; it is the non-mutating form to use in a deployment script.

`sandbox-resource-backend` is a **runtime probe**, not a static claim. It
reports what this host can enforce right now. The same trust decision —
`eggwork_runner::verify_trusted_helper` — backs that check, capability
advertisement, and admission, so the node never claims isolation its own
enforcement path would refuse.

A helper is trusted when it is root- or effective-user-owned, not group/world
writable, and inside non-writable ancestor directories. Anything else fails
closed: required isolation stays unsupported instead of quietly degrading.

## Drain

```bash
eggworkd drain   --config <node.json>   # {"draining": true,  "persistent": true}
eggworkd undrain --config <node.json>   # {"draining": false, "persistent": true}
```

Drain is stored beside the execution database and is observed by a running node
*before* it accepts each new execution. A draining node finishes in-flight work
and refuses new work; it does not kill anything.

Drain is **persistent** — it survives a restart. That is deliberate: it is what
makes it safe to drain, replace binaries, and then investigate without having
raced the node coming back up. Clear it explicitly when you are done.

## Garbage collection

`gc` is a preview by default and reads only:

```bash
eggworkd gc --config <node.json> --limit 128
# ... "dry_run": true
```

Applying it needs `--apply`, which is bounded by `--limit` and takes an
**exclusive lock on node state**. It therefore fails while the node process is
running — deliberately, because a live server is holding references the local
GC cannot see:

```bash
eggworkd gc --config <node.json> --apply --limit 128
# eggworkd: bounded garbage collection failed
```

A live service should collect through `NodeServer::collect_garbage` instead,
which operates on the running server's own stores and preserves active
references.

## Inspection and history

```bash
eggworkd executions list --config <node.json> --limit 50 --offset 0
eggworkd executions show  --config <node.json> <execution-id>
eggworkd inspect blob|workspace|artifact <id> --config <node.json>
```

`executions list` is paginated; pass the returned `next_offset` back as
`--offset` to continue. An unknown id returns a typed refusal
(`resource was not found or could not be inspected`) rather than a generic
error, and never reveals whether a different id exists — authorization runs
before resource resolution, so a denied caller cannot distinguish an existing
resource from an absent one.

## Redaction

`config print` prints the effective config with the private key path and every
client certificate fingerprint replaced by `[REDACTED]`, so the output is safe
to paste into a ticket.

## Configuration security

The loader is strict, and the failure mode is deliberately unhelpful — a single
`configuration is invalid or unreadable`. The requirements:

- The config file must be owned by the invoking user (or root) and **not**
  group- or world-writable.
- Every TLS file is checked the same way; the private key must additionally be
  unreadable by others (`0600`).
- `execution_root`, `blob_root`, `workspace_root` and the database's parent
  must be owner-only directories (`0700`).
- The config is capped in size and rejects symlinks and unknown fields.

The [quickstart](quickstart.md#5-set-permissions-required)
has the exact `chmod` incantation.

## Service management

Service verbs require an explicit product policy for the platform — Linux
`--unit-path`/`--scope`, macOS `--plist-path`/`--launchd-domain`/`--launchd-target`.
See [deployment.md](deployment.md).

On **Windows every mutating `service` verb is refused before any mutation**,
because `eggworkd` implements no service-control dispatcher; a registered
service can never reach `Running`, and registering anyway would leave a working
installation with no service behind it. Windows binary installation and daemon
runtime are unaffected.

## Design reference

[architecture/operations-cli.md](../architecture/operations-cli.md) ·
[architecture/server-node.md](../architecture/server-node.md) ·
[architecture/data-plane.md](../architecture/data-plane.md)
