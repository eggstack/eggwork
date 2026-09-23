# Security M002 Corrective — Landlock Runtime `/dev/null` Read

Status: closing

Source roadmap:

- `plans/subsystems/security-isolation-resource-roadmap.md`

Historical closure requiring correction:

- `plans/closure/security-isolation-resource/002-status.md`

## Finding

The M003 cancellation-under-limits fixture runs a background shell process.
Dash opens `/dev/null` for background stdin, which the original `workspace_rw`
runtime allowlist did not permit. The target therefore failed before the
cancellation path was exercised. M002's process-tree closure evidence did not
cover this common shell runtime dependency.

## Objective and scope

Allow read-only access to `/dev/null` in the trusted Landlock helper's runtime
allowlist and add a regression that executes a sandboxed background child,
cancels it, and proves the process tree is reaped. Grant no other `/dev` access.

## Acceptance

- `/dev/null` is the only new runtime path and receives only `ReadFile`;
- a sandboxed shell can start a background process using `/dev/null` for stdin;
- cancellation/timeout stops the helper and target process group;
- the original M002 closure record remains unchanged; this corrective record
  becomes the current evidence for the runtime dependency.

## Closure evidence

Create `plans/closure/security-isolation-resource/002a-status.md` with the
regression command, actual host/runtime evidence, and implementation commit.
