# Quickstart

Build a node from source, give it real TLS material, and prove it executes a
sandboxed request. Every command on this page has been run against this
repository at `0.1.5`; the output shown is real.

Expect about five minutes. The TLS material is the only fiddly part, and
step 2 does it for you.

## 1. Build the two Linux binaries

```bash
cargo build --locked -p eggwork-server --bin eggworkd
cargo build --locked -p eggwork-sandbox-helper
```

Linux ships two binaries because the daemon delegates Landlock confinement to a
separate helper. macOS and Windows ship the daemon only — Landlock is Linux-only,
so there is nothing for the helper to apply.

Check them:

```bash
./target/debug/eggworkd version
# {"version": "0.1.5"}
./target/debug/eggwork-sandbox-helper --version
# 0.1.5
```

## 2. Create TLS material

A node requires mTLS in both directions and will not start without it. The node
presents a server certificate; every controller must present a client
certificate signed by a CA the node trusts.

```bash
ROOT=$HOME/.local/share/eggwork
mkdir -p "$ROOT"/{tls,state,blobs,workspaces,executions,bin}

cd "$ROOT/tls"
openssl req -x509 -newkey rsa:2048 -nodes -keyout ca-key.pem -out ca.pem -days 30 \
  -subj "/CN=eggwork-controller-ca" \
  -config <(printf '[req]\ndistinguished_name=dn\nx509_extensions=v3\n[dn]\n[v3]\nbasicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\nsubjectKeyIdentifier=hash\n')
```

Now the node's server identity:

```bash
openssl req -newkey rsa:2048 -nodes -keyout node-key.pem -out node.csr -subj "/CN=eggwork-node"
printf 'basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:localhost,IP:127.0.0.1\n' > server.ext
openssl x509 -req -in node.csr -CA ca.pem -CAkey ca-key.pem -CAcreateserial \
  -out node.pem -days 30 -extfile server.ext
cat node.pem ca.pem > node-chain.pem
```

And a controller identity:

```bash
openssl req -newkey rsa:2048 -nodes -keyout client-key.pem -out client.csr -subj "/CN=eggwork-controller"
printf 'basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=clientAuth\n' > client.ext
openssl x509 -req -in client.csr -CA ca.pem -CAkey ca-key.pem -CAcreateserial \
  -out client.pem -days 30 -extfile client.ext
```

Eggwork identifies a controller by the **SHA-256 of its certificate's DER
encoding**, lowercase hex, no colons. Compute it exactly this way:

```bash
FP=$(openssl x509 -in client.pem -outform DER | openssl dgst -sha256 -hex | sed 's/^.*= //')
echo "$FP"
# e5f5ed1f65af60579208b93b5cfc880f997f6264062113b0980bb350d8272be6
```

## 3. Install the helper where it is trusted

```bash
install -m 0755 /path/to/repo/target/debug/eggwork-sandbox-helper \
  "$HOME/.local/lib/eggwork/eggwork-sandbox-helper"
```

The helper is trusted only when it is root- or effective-user-owned, not
group/world writable, **and every ancestor directory is non-writable**. That
last clause is why a helper under `/tmp` is rejected: `/tmp` is world-writable,
so anything inside it fails the walk. Install it somewhere like
`~/.local/lib/eggwork`, never in a shared temporary directory.

## 4. Write the config

```bash
cat > "$ROOT/node.json" <<EOF
{
  "schema_version": 1,
  "node_id": "worker-01",
  "bind": "127.0.0.1:7443",
  "execution_root": "$ROOT/executions",
  "database_path": "$ROOT/state/executions.sqlite",
  "blob_root": "$ROOT/blobs",
  "blob_quota_bytes": 1073741824,
  "workspace_root": "$ROOT/workspaces",
  "workspace_quota_bytes": 1073741824,
  "max_active_executions": 4,
  "lease_ttl_seconds": 30,
  "sandbox_helper": "$HOME/.local/lib/eggwork/eggwork-sandbox-helper",
  "tls": {
    "certificate_chain": "$ROOT/tls/node-chain.pem",
    "private_key": "$ROOT/tls/node-key.pem",
    "client_ca": "$ROOT/tls/ca.pem"
  },
  "clients": [
    {
      "principal_id": "controller-01",
      "certificate_sha256": "$FP",
      "operations": [
        "capabilities", "status", "execute", "observe", "cancel",
        "renew", "events", "blob-read", "blob-write",
        "workspace-create", "artifact-read"
      ]
    }
  ]
}
EOF
```

The field shape is also available as a committed example:
[`examples/eggworkd.example.json`](../examples/eggworkd.example.json). Replace
its paths and fingerprint with your own.

## 5. Set permissions (required)

Eggwork refuses to load a config or certificate that others could tamper with,
and refuses a state directory that others could write. Getting this wrong
produces the single unhelpful message `configuration is invalid or unreadable`.

```bash
chmod 700 "$ROOT" "$ROOT"/{state,blobs,workspaces,executions,tls}
chmod 600 "$ROOT"/node.json "$ROOT"/tls/{node-key.pem,ca-key.pem,client-key.pem}
chmod 644 "$ROOT"/tls/{node-chain.pem,node.pem,ca.pem,client.pem}
```

The rule in one line: **directories owner-only (`0700`), the private key
`0600`, and nothing group- or world-writable.**

## 6. Validate, then run

```bash
cd /path/to/repo
./target/debug/eggworkd doctor --config "$ROOT/node.json"
```

A ready node reports every check green:

```json
{
  "ready": true,
  "checks": [
    { "name": "configuration", "ok": true, "detail": "configuration and TLS policy are valid" },
    { "name": "sandbox-resource-backend", "ok": true,
      "detail": "runtime-probed capabilities: isolation.landlock.workspace-rw.v1, resources.cgroups-v2.memory, resources.cgroups-v2.cpu, resources.cgroups-v2.pids" },
    { "name": "listener", "ok": true, "detail": "bind address is available" },
    { "name": "execution-root", "ok": true, "detail": "directory is available" },
    { "name": "blob-root", "ok": true, "detail": "directory is available" },
    { "name": "workspace-root", "ok": true, "detail": "directory is available" },
    { "name": "storage-headroom", "ok": true, "detail": "116914479104 bytes available" }
  ]
}
```

The `sandbox-resource-backend` line is the important one: it is a **runtime
probe**, not a static claim. It reports the capabilities this host can actually
enforce right now. If the helper is untrusted, that line fails and required
isolation is refused rather than degraded.

Start the node:

```bash
./target/debug/eggworkd run --config "$ROOT/node.json"
# {"local_addr":"127.0.0.1:7443","node_id":"worker-01","ready":true}
```

`ready: true` is the node listening and serving. In another terminal:

```bash
./target/debug/eggworkd status --config "$ROOT/node.json"
./target/debug/eggworkd executions list --config "$ROOT/node.json" --limit 5
./target/debug/eggworkd storage summary --config "$ROOT/node.json"
```

## 7. Prove an execution is actually confined

This is the step worth doing. It runs one authenticated execution end to end —
mTLS client, admission, runner, Landlock sandbox — and then tries to write
*outside* the workspace to confirm the sandbox stopped it.

> **Stop the node first.** The check below takes an exclusive lock on the state
> directory, so it fails if an `eggworkd` is already running against the same
> `EGGWORK_QUALIFY_ROOT`. If you ran step 6, stop it first: `pkill -x eggworkd`.

Materialize the fixture (it mints its own TLS and node config under
`$EGGWORK_QUALIFY_ROOT`):

```bash
EGGWORK_QUALIFY_ROOT="$ROOT" \
EGGWORK_QUALIFY_HELPER="$HOME/.local/lib/eggwork/eggwork-sandbox-helper" \
  cargo test -p eggwork-server --features qualification \
  --test installed_qualification -- --ignored --exact qualification_fixture
```

Then run the execution against it:

```bash
EGGWORK_QUALIFY_DAEMON=/path/to/repo/target/debug/eggworkd \
EGGWORK_QUALIFY_CONFIG="$ROOT/state/node.json" \
EGGWORK_QUALIFY_TLS_DIR="$ROOT/state/tls" \
EGGWORK_QUALIFY_HELPER="$HOME/.local/lib/eggwork/eggwork-sandbox-helper" \
  cargo test -p eggwork-server --features qualification \
  --test installed_qualification -- --ignored \
  --exact installed_release_admits_execution_and_refuses_unsupported_isolation --nocapture
```

The receipt includes:

```json
{
  "state": "Succeeded",
  "stdout": "eggwork-qualification\n",
  "isolation_capable": true,
  "required_isolation": {
    "advertised": true,
    "applied": { "profile": "workspace_rw", "status": "applied" },
    "outside_workspace_write_denied": true
  }
}
```

`outside_workspace_write_denied: true` is the load-bearing field: the sandbox
was applied *and* an escape attempt was refused. `isolation_capable: true` on
its own would only mean the node could enforce isolation.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `configuration is invalid or unreadable` | Wrong permissions, or the file is unreadable | `chmod 600` the config; see step 5 |
| `doctor` says `configuration and TLS policy are valid` is false | A certificate is malformed, or expired | Re-check the chain; `openssl x509 -in ca.pem -noout -text` |
| `sandbox-resource-backend` fails | Helper missing, group/world writable, or under a world-writable ancestor | Move it to `~/.local/lib/eggwork` (step 3) |
| `listener` fails | Port already bound | Change `bind`, or stop the other process |
| `configuration ... rejected` right after a `drain` | Not an error — drain persists across restarts until `undrain` | `eggworkd undrain --config …` |
| The step 7 execution test fails immediately | A node is still running against the same state root and holds the lock | `pkill -x eggworkd`, then re-run |

## Next

- [operations.md](operations.md) — the full operator verb reference.
- [deployment.md](deployment.md) — install and update a node as a service.
- [releases.md](releases.md) — consume a published release.
