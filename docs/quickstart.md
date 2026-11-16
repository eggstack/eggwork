# Quickstart

Build and run a Linux node from source, then verify an authenticated execution.
The qualification check also proves that required isolation is enforced when
the host supports it, and refused when it does not. Use the
[release guide](releases.md) for published binaries.

You need Linux, Rust through `rustup`, OpenSSL, Python 3, and a host where the
node can access its execution and storage directories. The repository pins the
Rust toolchain in `rust-toolchain.toml`. These commands use Bash. Start from the
repository root, replace `/path/to/repo` with its absolute path, and keep the
same shell open through step 7 so `$ROOT` and `$FP` remain available.

## 1. Build the two Linux binaries

```bash
cargo build --locked -p eggwork-server --bin eggworkd
cargo build --locked -p eggwork-sandbox-helper
```

Linux ships two binaries because the daemon delegates Landlock confinement to a
separate helper. macOS and Windows ship the daemon only. Landlock is Linux-only,
so those platforms do not use the helper.

Check their version reports:

```bash
./target/debug/eggworkd version
./target/debug/eggwork-sandbox-helper --version
```

## 2. Create TLS material

The walkthrough writes test data under a separate `eggwork-quickstart`
directory. If that path already contains data from another node, change `ROOT`
to a new empty directory before running these commands.

A node requires mTLS in both directions and will not start without it. The node
presents a server certificate; every controller must present a client
certificate signed by a CA the node trusts. These short-lived self-signed
certificates are for local testing. Use your organization's certificate
authority and key-management practices for a deployed node.

```bash
ROOT=$HOME/.local/share/eggwork-quickstart
mkdir -p "$ROOT"/{tls,state,blobs,workspaces,executions}

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

Eggwork identifies a controller by the SHA-256 of its certificate's DER
encoding, written as lowercase hex without colons:

```bash
FP=$(openssl x509 -in client.pem -outform DER | openssl dgst -sha256 -hex | sed 's/^.*= //')
echo "$FP"
```

## 3. Install the helper where it is trusted

```bash
install -d -m 0755 "$HOME/.local/lib/eggwork-quickstart"
chmod 0755 "$HOME/.local/lib/eggwork-quickstart"
install -m 0755 /path/to/repo/target/debug/eggwork-sandbox-helper \
  "$HOME/.local/lib/eggwork-quickstart/eggwork-sandbox-helper"
```

The helper file and its containing directory must be owned by root or the
effective user and must not be group-writable or world-writable. Ancestors are
checked too. Keep the helper in `~/.local/lib/eggwork-quickstart`, not directly
under `/tmp`.

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
  "sandbox_helper": "$HOME/.local/lib/eggwork-quickstart/eggwork-sandbox-helper",
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

Host telemetry is optional and disabled unless configured. See
[node operations](operations.md#host-telemetry) if you use a local provider.

## 5. Set permissions (required)

Eggwork refuses unsafe config, TLS, or state permissions. Errors name the rule
that failed and the corrective action:

```text
eggworkd: [config_file_world_writable] /srv/eggwork/node.json: remove group- and world-write permission from the configuration file
```

```bash
chmod 700 "$ROOT" "$ROOT"/{state,blobs,workspaces,executions,tls}
chmod 600 "$ROOT"/node.json "$ROOT"/tls/{node-key.pem,ca-key.pem,client-key.pem}
chmod 644 "$ROOT"/tls/{node-chain.pem,node.pem,ca.pem,client.pem}
```

Use owner-only (`0700`) directories, a private key readable only by its owner
(`0600`), and no group- or world-writable config or TLS files.

## 6. Validate, then run

```bash
cd /path/to/repo
./target/debug/eggworkd doctor --config "$ROOT/node.json"
```

A ready node reports `ready: true` and all eight checks as `ok: true`. The
following excerpt omits each check's detail field:

```json
{
  "schema_version": 1,
  "ready": true,
  "checks": [
    { "name": "configuration", "ok": true },
    { "name": "host-telemetry", "ok": true },
    { "name": "sandbox-resource-backend", "ok": true },
    { "name": "listener", "ok": true },
    { "name": "execution-root", "ok": true },
    { "name": "blob-root", "ok": true },
    { "name": "workspace-root", "ok": true },
    { "name": "storage-headroom", "ok": true }
  ]
}
```

`sandbox-resource-backend` reports the capabilities this host can enforce now.
If the helper is untrusted, the check fails and required isolation is refused.

Start the node:

```bash
./target/debug/eggworkd run --config "$ROOT/node.json"
# {"local_addr":"127.0.0.1:7443","node_id":"worker-01","ready":true}
```

`ready: true` means the node is listening. In another terminal, run the
following commands. If you changed `ROOT`, update the config path accordingly:

```bash
/path/to/repo/target/debug/eggworkd status --config "$HOME/.local/share/eggwork-quickstart/node.json"
/path/to/repo/target/debug/eggworkd executions list --config "$HOME/.local/share/eggwork-quickstart/node.json" --limit 5
/path/to/repo/target/debug/eggworkd storage summary --config "$HOME/.local/share/eggwork-quickstart/node.json"
```

## 7. Verify execution and isolation

This qualification check uses the production client over mTLS and submits a
bounded execution. If the node advertises Landlock isolation, the check also
tries to write outside the workspace and confirms that the write is denied. If
the host does not support isolation, it confirms that a request requiring it
is refused before the target runs.

Stop the node started in step 6 with Ctrl-C before continuing. The check takes
an exclusive lock on the state directory and cannot run while the node is
using the same `EGGWORK_QUALIFY_ROOT`.

Materialize the fixture (it mints its own TLS and node config under
`$EGGWORK_QUALIFY_ROOT`):

```bash
EGGWORK_QUALIFY_ROOT="$ROOT" \
EGGWORK_QUALIFY_HELPER="$HOME/.local/lib/eggwork-quickstart/eggwork-sandbox-helper" \
  cargo test --locked -p eggwork-server --features qualification \
  --test installed_qualification -- --ignored --exact qualification_fixture
```

Then run the execution against it:

```bash
EGGWORK_QUALIFY_DAEMON=/path/to/repo/target/debug/eggworkd \
EGGWORK_QUALIFY_CONFIG="$ROOT/state/node.json" \
EGGWORK_QUALIFY_TLS_DIR="$ROOT/state/tls" \
EGGWORK_QUALIFY_HELPER="$HOME/.local/lib/eggwork-quickstart/eggwork-sandbox-helper" \
  cargo test --locked -p eggwork-server --features qualification \
  --test installed_qualification -- --ignored \
  --exact installed_release_admits_execution_and_refuses_unsupported_isolation --nocapture
```

On a host that supports Landlock, the receipt includes:

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
| `[config_file_world_writable] …` on stderr | The config file is group- or world-writable | `chmod 600` the config; see step 5 |
| `[tls_private_key_readable_by_others] the private key` | The private key is readable by other users | `chmod 600` the private key; see step 5 |
| `[state_root_not_owner_only] …` | A state directory is not `0700` | `chmod 700` the state directories; see step 5 |
| `doctor` says `configuration and TLS policy are valid` is false | A certificate is malformed, or expired | Check the certificates with `openssl verify -CAfile "$ROOT/tls/ca.pem" "$ROOT/tls/node.pem" "$ROOT/tls/client.pem"` |
| `sandbox-resource-backend` fails | Helper missing, group/world writable, or under an untrusted ancestor | Install it under `~/.local/lib/eggwork-quickstart` (step 3) |
| `listener` fails | Port already bound | Change `bind`, or stop the other process |
| `configuration ... rejected` right after a `drain` | Expected: drain persists across restarts until `undrain` | `eggworkd undrain --config <node.json>` |
| The step 7 execution test fails immediately | A node is still running against the same state root and holds the lock | Stop that node with Ctrl-C, then re-run |

## Next

- [Node operations](operations.md): readiness, drain, inspection, and garbage collection.
- [Deployment](deployment.md): install and update a node as a service.
- [Releases](releases.md): supported targets and release installation.
