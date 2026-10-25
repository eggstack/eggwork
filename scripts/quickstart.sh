#!/usr/bin/env bash
#
# scripts/quickstart.sh — the machine-executed form of docs/quickstart.md.
#
# Operations M008b §5.4 requires the quickstart to be machine-verified rather
# than merely asserted to be true. This script is that machine-checked form: a
# CI job runs it end to end on Linux against a locally built node with
# generated TLS material, and `scripts/check_quickstart_sync.py` compares the
# command blocks in `docs/quickstart.md` with the commands below so the
# document cannot drift from what CI actually runs.
#
# The two forms are kept honest by a marker convention:
#
#   # step: <id>            names the step the region belongs to
#   # doc:begin ... # doc:end
#                           the commands that `docs/quickstart.md` must show
#
# Everything outside a `# doc:begin`/`# doc:end` region is *plumbing* — the
# script's own bookkeeping. The document is not required to show it, and the
# sync check does not compare it.
#
# Failure output names the failing step, so a red CI job is diagnosable from
# the log alone:
#
#   === quickstart step 6: Validate, then run ===
#   ...
#   quickstart FAILED at step 6 (Validate, then run), exit status 1
#
# Environment overrides (all optional):
#
#   CARGO_TARGET_DIR                   where the binaries are built and run
#   EGGWORK_QUICKSTART_ROOT            installation root (default ~/.local/share/eggwork)
#   EGGWORK_QUICKSTART_HELPER_DIR      trusted helper directory (default ~/.local/lib/eggwork)
#   EGGWORK_QUICKSTART_SKIP_QUALIFY   set to 1 to skip step 7 (local convenience only)

set -euo pipefail
set -E

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
DAEMON="$TARGET_DIR/debug/eggworkd"
HELPER_BINARY="$TARGET_DIR/debug/eggwork-sandbox-helper"
HELPER_DIR="${EGGWORK_QUICKSTART_HELPER_DIR:-$HOME/.local/lib/eggwork}"
HELPER="$HELPER_DIR/eggwork-sandbox-helper"
DOC="$REPO_ROOT/docs/quickstart.md"
SYNC_CHECK="$REPO_ROOT/scripts/check_quickstart_sync.py"
WORK_DIR="${EGGWORK_QUICKSTART_WORK_DIR:-$REPO_ROOT/target/quickstart}"

CURRENT_STEP="startup"
CURRENT_STEP_TITLE="Quickstart"
NODE_PID=""

step() {
    CURRENT_STEP="$1"
    CURRENT_STEP_TITLE="$2"
    printf '\n=== quickstart step %s: %s ===\n' "$CURRENT_STEP" "$CURRENT_STEP_TITLE"
}

# The node is stopped on every failure path, so a failed step never leaves a
# process holding the state directory's lock.
cleanup_node() {
    if [ -n "$NODE_PID" ] && kill -0 "$NODE_PID" 2>/dev/null; then
        kill -INT "$NODE_PID" 2>/dev/null || true
        wait "$NODE_PID" 2>/dev/null || true
    fi
}

on_failure() {
    local status="$1"
    printf '\nquickstart FAILED at step %s (%s), exit status %s\n' \
        "$CURRENT_STEP" "$CURRENT_STEP_TITLE" "$status" >&2
    exit "$status"
}
trap 'status=$?; cleanup_node; on_failure "$status"' ERR

require_command() {
    command -v "$1" >/dev/null 2>&1 || {
        printf 'quickstart FAILED at step %s (%s): required command not found: %s\n' \
            "$CURRENT_STEP" "$CURRENT_STEP_TITLE" "$1" >&2
        exit 1
    }
}

# A path is trusted only when it is not group- or world-writable and no
# ancestor directory is. The node re-checks this; the script checks it too so
# that a mis-placed helper fails with a message about *this* script rather
# than about the node's trust verdict.
#
# The function succeeds when the path is *not* writable by others, so the
# caller negates it: a directory that fails this test is untrusted.
is_not_writable_by_others() {
    local mode other group
    mode="$(stat -c '%a' "$1")"
    other="${mode: -1}"
    group="${mode: -2:1}"
    [ $(( 8#$other & 2 )) -eq 0 ] && [ $(( 8#$group & 2 )) -eq 0 ]
}

check_trusted_path() {
    local target="$1"
    local directory
    directory="$(dirname "$target")"
    while [ "$directory" != "/" ] && [ -n "$directory" ]; do
        if [ -d "$directory" ] && ! is_not_writable_by_others "$directory"; then
            printf 'quickstart FAILED at step %s (%s): %s is under a group- or world-writable directory (%s)\n' \
                "$CURRENT_STEP" "$CURRENT_STEP_TITLE" "$target" "$directory" >&2
            exit 1
        fi
        directory="$(dirname "$directory")"
    done
}

require_command cargo
require_command openssl
require_command python3
require_command stat

mkdir -p "$WORK_DIR"

# ---------------------------------------------------------------------------
# Step 1 — build the two Linux binaries
# ---------------------------------------------------------------------------
step "1" "Build the two Linux binaries"
cd "$REPO_ROOT"

# step: 1-build
# doc:begin
cargo build --locked -p eggwork-server --bin eggworkd
cargo build --locked -p eggwork-sandbox-helper
# doc:end

# step: 1-versions
# doc:begin
"$DAEMON" version
"$HELPER_BINARY" --version
# doc:end

# ---------------------------------------------------------------------------
# Step 2 — create TLS material
# ---------------------------------------------------------------------------
step "2" "Create TLS material"

# step: 2-tls
# doc:begin
ROOT="${EGGWORK_QUICKSTART_ROOT:-$HOME/.local/share/eggwork}"
mkdir -p "$ROOT"/{tls,state,blobs,workspaces,executions,bin}

cd "$ROOT/tls"
openssl req -x509 -newkey rsa:2048 -nodes -keyout ca-key.pem -out ca.pem -days 30 \
  -subj "/CN=eggwork-controller-ca" \
  -config <(printf '[req]\ndistinguished_name=dn\nx509_extensions=v3\n[dn]\n[v3]\nbasicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\nsubjectKeyIdentifier=hash\n')
# doc:end

# step: 2-node-identity
# doc:begin
openssl req -newkey rsa:2048 -nodes -keyout node-key.pem -out node.csr -subj "/CN=eggwork-node"
printf 'basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:localhost,IP:127.0.0.1\n' > server.ext
openssl x509 -req -in node.csr -CA ca.pem -CAkey ca-key.pem -CAcreateserial \
  -out node.pem -days 30 -extfile server.ext
cat node.pem ca.pem > node-chain.pem
# doc:end

# step: 2-controller-identity
# doc:begin
openssl req -newkey rsa:2048 -nodes -keyout client-key.pem -out client.csr -subj "/CN=eggwork-controller"
printf 'basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=clientAuth\n' > client.ext
openssl x509 -req -in client.csr -CA ca.pem -CAkey ca-key.pem -CAcreateserial \
  -out client.pem -days 30 -extfile client.ext
# doc:end

# step: 2-fingerprint
# doc:begin
FP=$(openssl x509 -in client.pem -outform DER | openssl dgst -sha256 -hex | sed 's/^.*= //')
echo "$FP"
# doc:end

# ---------------------------------------------------------------------------
# Step 3 — install the helper where it is trusted
# ---------------------------------------------------------------------------
step "3" "Install the helper where it is trusted"
# The helper's directory is created and then explicitly tightened. `mkdir -p`
# would inherit the ambient umask, and a group-writable directory makes the
# helper untrusted — the node would then refuse the isolation it is required
# to enforce, and the refusal would look like a product defect.
install -d -m 0755 "$HELPER_DIR"
chmod 0755 "$HELPER_DIR"
check_trusted_path "$HELPER"

# step: 3-install-helper
# doc:begin
install -m 0755 "$HELPER_BINARY" "$HELPER"
# doc:end

# ---------------------------------------------------------------------------
# Step 4 — write the config
# ---------------------------------------------------------------------------
step "4" "Write the config"

# step: 4-config
# doc:begin
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
  "sandbox_helper": "$HELPER_DIR/eggwork-sandbox-helper",
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
# doc:end

# ---------------------------------------------------------------------------
# Step 5 — set permissions (required)
# ---------------------------------------------------------------------------
step "5" "Set permissions (required)"

# step: 5-permissions
# doc:begin
chmod 700 "$ROOT" "$ROOT"/{state,blobs,workspaces,executions,tls}
chmod 600 "$ROOT"/node.json "$ROOT"/tls/{node-key.pem,ca-key.pem,client-key.pem}
chmod 644 "$ROOT"/tls/{node-chain.pem,node.pem,ca.pem,client.pem}
# doc:end

# ---------------------------------------------------------------------------
# Step 6 — validate, then run
# ---------------------------------------------------------------------------
step "6" "Validate, then run"

DOCTOR_REPORT="$WORK_DIR/doctor.json"

# The captured report is what the document's *shown* doctor output is compared
# against. The plan records that the document currently shows seven checks
# and the node emits eight; this comparison is what would have caught it.
"$DAEMON" doctor --config "$ROOT/node.json" >"$DOCTOR_REPORT" 2>&1 || {
    printf 'quickstart FAILED at step %s (%s): doctor reported an unhealthy node\n' \
        "$CURRENT_STEP" "$CURRENT_STEP_TITLE" >&2
    cat "$DOCTOR_REPORT" >&2
    exit 1
}
python3 "$SYNC_CHECK" --doctor-report "$DOCTOR_REPORT"

# step: 6-doctor
# doc:begin
cd "$REPO_ROOT"
"$DAEMON" doctor --config "$ROOT/node.json"
# doc:end

# step: 6-run
# The node is started in the background so that the operator commands below
# run against a live node. The document shows the same command run in the
# foreground in a second terminal; the trailing `&` is a scheduling modifier,
# not a different command, and the sync check ignores it.
# doc:begin
"$DAEMON" run --config "$ROOT/node.json" &
# doc:end

NODE_PID=$!

# The node is ready when it serves: its store is open and its listener
# accepts. Polling with the operator's own `status` command and a loopback
# connection avoids inventing a second readiness definition.
BIND="$(sed -n 's/.*"bind": "\([^"]*\)".*/\1/p' "$ROOT/node.json")"
ready=0
for _ in $(seq 1 60); do
    if "$DAEMON" status --config "$ROOT/node.json" >/dev/null 2>&1 &&
        (exec 3<>"/dev/tcp/${BIND%:*}/${BIND##*:}") 2>/dev/null; then
        ready=1
        break
    fi
    if ! kill -0 "$NODE_PID" 2>/dev/null; then
        printf 'quickstart FAILED at step %s (%s): the node exited before it became ready; its output is above\n' \
            "$CURRENT_STEP" "$CURRENT_STEP_TITLE" >&2
        exit 1
    fi
    sleep 1
done
if [ "$ready" -ne 1 ]; then
    printf 'quickstart FAILED at step %s (%s): the node did not become ready\n' \
        "$CURRENT_STEP" "$CURRENT_STEP_TITLE" >&2
    exit 1
fi

# step: 6-inspect
# doc:begin
"$DAEMON" status --config "$ROOT/node.json"
"$DAEMON" executions list --config "$ROOT/node.json" --limit 5
"$DAEMON" storage summary --config "$ROOT/node.json"
# doc:end

# The node holds the state directory; the qualification fixture takes an
# exclusive lock on it, so the node must be stopped first.
if kill -0 "$NODE_PID" 2>/dev/null; then
    kill -INT "$NODE_PID"
    wait "$NODE_PID" || true
fi
NODE_PID=""

# ---------------------------------------------------------------------------
# Step 7 — prove an execution is actually confined
# ---------------------------------------------------------------------------
step "7" "Prove an execution is actually confined"

if [ "${EGGWORK_QUICKSTART_SKIP_QUALIFY:-0}" = "1" ]; then
    printf 'quickstart step 7 skipped (EGGWORK_QUICKSTART_SKIP_QUALIFY=1)\n'
else
    # step: 7-fixture
    # doc:begin
    EGGWORK_QUALIFY_ROOT="$ROOT" \
    EGGWORK_QUALIFY_HELPER="$HELPER" \
      cargo test --locked -p eggwork-server --features qualification \
      --test installed_qualification -- --ignored --exact qualification_fixture
    # doc:end

    # step: 7-execution
    # doc:begin
    EGGWORK_QUALIFY_DAEMON="$DAEMON" \
    EGGWORK_QUALIFY_CONFIG="$ROOT/state/node.json" \
    EGGWORK_QUALIFY_TLS_DIR="$ROOT/state/tls" \
    EGGWORK_QUALIFY_HELPER="$HELPER" \
      cargo test --locked -p eggwork-server --features qualification \
      --test installed_qualification -- --ignored \
      --exact installed_release_admits_execution_and_refuses_unsupported_isolation --nocapture
    # doc:end
fi

# ---------------------------------------------------------------------------
# The document and this script must agree
# ---------------------------------------------------------------------------
step "8" "Check the document against this script"
python3 "$SYNC_CHECK"

printf '\nquickstart completed successfully\n'
