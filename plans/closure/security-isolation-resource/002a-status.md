# Security M002a Closure — Landlock Runtime `/dev/null` Read

Status: closed

Reviewed implementation commit: `26f42bb84533fd9d77812377e4603d591810a12c`

Corrective implementation plan: `plans/implementation/security-isolation-resource/002a-landlock-runtime-devnull.md`

Historical M002 closure: `plans/closure/security-isolation-resource/002-status.md` (preserved unchanged).

## Requirement-to-evidence matrix

| Requirement | Evidence | Result |
|---|---|---|
| Add only `/dev/null`, with read-only Landlock access | `crates/eggwork-sandbox-helper/src/main.rs` runtime allowlist change; `landlock_runner::cancellation_and_timeout_clean_up_process_trees` executes a background shell child under the sandbox | Pass |
| Background shell process can start and cancellation reaps its process tree | `rtk cargo test -p eggwork-sandbox-helper --test landlock_runner`: 10 passed; cancellation fixture waits for child marker, cancels the execution, and checks the child is reaped | Pass on Linux x86_64 |
| Preserve original M002 history | Original M002 closure record was not modified; this corrective record is the current evidence for the additional runtime dependency | Pass |

## Verification and platform evidence

- Host: Linux x86_64, kernel `6.8.0-139-generic`.
- Command: `rtk cargo test -p eggwork-sandbox-helper --test landlock_runner` — 10 passed.
- Broader command: `rtk cargo test --workspace --all-targets` — 67 passed.
- `rtk cargo clippy --workspace --all-targets -- -D warnings` — passed.
- `rtk git diff --check` — passed.

The runtime change grants the helper's sanitized target only read access to `/dev/null`; it does not grant access to other `/dev` paths. Linux Landlock behavior was exercised on the host above. Other operating systems were not exercised by this correction.

## Review and disposition

The initially observed dash background-job failure (`cannot open /dev/null: Permission denied`) is covered by the regression fixture. The corrected fixture now reaches the cancellation path and verifies descendant cleanup. No unresolved findings remain for this corrective scope. Security M002a is closed; the original M002 record remains intact.
