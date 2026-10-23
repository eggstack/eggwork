# Protocol layering (pre-wire)

The core data model is protocol-neutral. Later control-plane work will define versioned operations over Eggstack transport with transport-derived principal identity. Large blobs will use a streaming data path rather than JSON expansion. The exact public wire schema is not frozen here.

See the [control-plane roadmap](../plans/subsystems/control-plane-protocol-roadmap.md) and [ADR-0002](../plans/adrs/ADR-0002-eggstack-transport-and-mtls.md).

Component deep dives: [server-node.md](server-node.md) (node side of the wire contract) and [client-transport.md](client-transport.md) (client side). Overview: [overview.md](overview.md).

## The node telemetry wire contract (schema version 1)

`GET /v1/node/telemetry` returns a `NodeTelemetry` document. This is the **frozen**
contract for the optional host-telemetry projection, and the reference for the
downstream CodeGG worker-telemetry consumer.

```json
{
  "schema_version": 1,
  "node_id": "worker-01",
  "source": "gregg",
  "state": "ready",
  "age_ms": 231,
  "last_error": null,
  "observation": {
    "observed_at_unix_ms": 1729460000123,
    "sample_interval_ms": 1000,
    "cpu": { "value": { "logical_cores": 8, "usage_pct": 12.5, "iowait_pct": 1.5 } },
    "memory": { "value": { "available_bytes": 4000000, "total_bytes": 8000000, "used_bytes": 4000000 } },
    "load": { "value": { "one": 0.5, "five": 0.25, "fifteen": 0.125 } },
    "swap": "unsupported",
    "disk": { "value": { "available_bytes": 900, "total_bytes": 1000, "volume_count": 1 } },
    "disk_io": { "value": { "read_bytes_per_sec": 11, "write_bytes_per_sec": 22 } },
    "network": { "value": { "rx_bytes_per_sec": 33, "tx_bytes_per_sec": 44 } }
  }
}
```

### Fields

| Field | Type | Meaning |
| --- | --- | --- |
| `schema_version` | `1` | The contract version. A reader refuses any other value rather than guessing. Additive changes stay within version 1. |
| `node_id` | string | The node that produced the observation. |
| `source` | `"gregg"` | The kind of provider the measurement came from. The set is closed; an unknown source is refused. |
| `state` | enum | What the *provider* is doing. See below. |
| `age_ms` | number or null | How long ago, in milliseconds, the node received the observation, measured on the node's monotonic clock. `null` when there is no observation. |
| `last_error` | enum or null | The bounded category of the most recent provider failure. `null` when the last interaction succeeded or none has failed. |
| `observation` | object or null | The most recent measurement the node holds. `null` when it holds none. |

### Provider states

`state` describes the provider, never the host. A node with no provider is
`disabled`, which is not a host with zero usage.

| `state` | `observation` | Meaning |
| --- | --- | --- |
| `disabled` | absent | No provider is configured. The node claims no host measurements. |
| `warming` | absent | The provider is alive but has not produced a sample yet. |
| `ready` | present | A sample is present and within its freshness bound. |
| `stale` | present | A sample is present but older than the freshness bound. |
| `unavailable` | present or absent | The provider could not be reached. |
| `invalid` | present or absent | The provider answered with something unusable. |

A sample is **current only when the state is `ready`**. A reader that needs a
fresh measurement must check the state, not merely the presence of an
observation. `unavailable` and `invalid` may still carry the last sample the
node took; `age_ms` says how old it is.

### Measurement groups

Each measurement group is one of:

| Form | Meaning |
| --- | --- |
| `{ "value": { ... } }` | A validated measurement. |
| `"unsupported"` | The host platform cannot produce this measurement. |
| `"unknown"` | The measurement is supported, but this sample carried no value. |

A missing measurement is **never zero**. A reader can tell "this host has no
swap" apart from "this host has swap and it is currently unused".

The groups are `cpu` (logical cores, busy percentage, optional I/O-wait
percentage), `memory` (available/total/used bytes), `load` (one/five/fifteen
minute load averages), `swap` (used/total bytes), `disk` (aggregate capacity
across the volumes the provider reported, with no mount path), `disk_io`
(aggregate read/write throughput), and `network` (aggregate receive/transmit
throughput).

### What the contract deliberately does not carry

No provider address, no mount path, no device or interface identity, no
hostname, no per-execution measurement, and no raw provider JSON. The node's
own `NodeStatus` and `NodeCapabilities` are unchanged: they answer "is this
node accepting work", and this document answers "what did this host look
like". The two are never merged, and nothing in this contract qualifies an
execution.

## The execution diagnostic payload (schema version 1)

`ExecutionEventKind::Diagnostic(String)` carries one diagnostic payload. The
payload is a single bounded string, not an object, because it rides inside an
event whose envelope is already bounded.

### Grammar

```text
eggwork.diagnostic.<class>.v1 key=value key=value …
```

* The prefix is `eggwork_core::DIAGNOSTIC_PREFIX` (`eggwork.diagnostic.`) and
  the schema suffix is `eggwork_core::DIAGNOSTIC_SCHEMA_SUFFIX` (`.v1`).
* The class is one of the closed set in `eggwork_core::DIAGNOSTIC_CLASSES`:
  `isolation.degraded` (`DIAGNOSTIC_CLASS_ISOLATION_DEGRADED`) and
  `resources.degraded` (`DIAGNOSTIC_CLASS_RESOURCES_DEGRADED`). A class
  outside the set is refused, not guessed at, because the class is what a
  consumer dispatches on.
* Fields are `key=value` pairs separated by single spaces. The key set is a
  lowercase ASCII token of at most `MAX_DIAGNOSTIC_KEY_BYTES` bytes; `class` is
  reserved and is derived from the payload's own class.
* Values are printable ASCII. Every character outside `!`..=`~` — a space
  included — becomes `_`, so a value can never contain the separator and the
  tokenisation stays unambiguous without an escaping scheme.

### Required keys

| Key | Meaning |
| --- | --- |
| `class` | The class, repeated as a field so a consumer that only reads the key/value pairs does not have to parse the prefix. |
| `requirement` | Always `best_effort`. A degradation is by definition a best-effort request that was not honoured; a required request is refused instead. |
| `reason` | The bounded, human-readable cause of the degradation. |
| `profile` | `isolation.degraded` only: the requested filesystem profile. |
| `dimension` | `resources.degraded` only: the affected dimension (`memory`, `cpu`, `pids`). |

`eggwork_core::DiagnosticPayload::parse` enforces the grammar, the closed
class set, the bound, and the presence of `class`, `requirement` and `reason`.

### Bound and truncation

The assembled payload never exceeds
`eggwork_core::MAX_DIAGNOSTIC_PAYLOAD_BYTES` (`MAX_METADATA_VALUE_BYTES`,
2048 bytes). When the fields do not fit, values are shortened in insertion
order: the first value that does not fit is cut on a UTF-8 character boundary
and gains the literal marker `eggwork_core::DIAGNOSTIC_TRUNCATION_MARKER`
(`...[truncated]`), and the fields after it are omitted. The marker is what
makes truncation observable — a bounded payload never silently reads as a
complete statement. The free-form `reason` is inserted last so that it
absorbs the overage and the structured keys survive intact.

### What the payload deliberately does not carry

No secret, no credential, no path outside the execution's own workspace
naming, no stack trace, and no per-dimension measurement. A diagnostic says
*what was not enforced and why*, in the smallest form that lets a caller act
on it. The terminal snapshot (`ExecutionResult.sandbox`,
`ExecutionResult.resources`) remains the authoritative evidence; the
diagnostic makes the same fact observable while the execution is still
running.
