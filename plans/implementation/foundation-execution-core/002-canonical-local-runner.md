# Foundation and Execution Core M002 — Canonical Local Runner

Status: blocked on Foundation M001 closure

Source roadmap:

- `plans/subsystems/foundation-execution-core-roadmap.md`

Canonical references:

- `plans/000-long-term-specification.md#7-execution-request-contract`
- `plans/000-long-term-specification.md#9-execution-lifecycle-and-terminal-result`
- `plans/adrs/ADR-0005-runner-service-separation-and-local-admission.md`

Research baseline:

- inspect current CodeGG `src/managed_process.rs` before implementation; planning baseline commit is in `plans/registry.md`.

## 1. Objective

Implement the one canonical Eggwork finite noninteractive process runner, generalizing proven CodeGG lifecycle semantics while keeping all CodeGG scheduler/domain ownership outside Eggwork.

## 2. Scope

In:

- argv execution;
- cwd under caller-provided already-authorized local root;
- sanitized environment;
- null/bounded bytes stdin;
- bounded stdout/stderr capture;
- bounded streaming chunks with backpressure;
- timeout;
- cancellation;
- process session/tree cleanup;
- typed termination;
- cleanup diagnostics;
- provenance;
- sandbox/resource request/outcome hooks without implementing full platform policy.

Out:

- HTTP/TLS;
- local admission;
- workspace transfer;
- global queue/scheduler;
- PTY;
- semantic retries.

## 3. Required production changes

1. Implement `eggwork-runner` service/trait with one asynchronous canonical path.
2. Add request translation from core ExecutionSpec-local command fields into a runner request.
3. Sanitize environment by default; explicitly deny command/loader/credential-altering variables identified by review.
4. Force noninteractive defaults where appropriate.
5. Start children in an independently controllable process group/session on Unix; implement corresponding Windows process-tree strategy or explicit platform limitation.
6. Drain stdout/stderr concurrently without unbounded accumulation.
7. Support head/tail bounded capture with omitted/total byte facts.
8. Stream bounded chunks through bounded channels; a slow consumer must not stop pipe draining or cause unbounded memory.
9. Race natural exit, timeout, cancellation, and output-limit termination deterministically.
10. Gracefully terminate then force terminate descendants according to platform policy.
11. Return cleanup diagnostics without rewriting an already-known process outcome.
12. Propagate neutral execution provenance through environment only where safe and documented.
13. Expose sandbox/resource setup hooks as typed requests/results; required hook failure will later fail closed.

## 4. CodeGG reuse constraints

Do reuse concepts/algorithms proven in CodeGG ManagedProcessService.

Do not import:

- JobId/AttemptId;
- CODEGG-specific environment variable names as the canonical Eggwork contract;
- scheduler types;
- AgentRun types;
- CodeGG permission models.

If code is copied/adapted, preserve license/provenance and review it as Eggwork-owned code.

## 5. Failure/cancellation semantics

Distinguish:

- invalid request before spawn;
- cancelled before spawn;
- spawn failure;
- wait/read/stream internal failure;
- natural success/nonzero exit;
- timeout;
- explicit cancellation;
- output-limit termination;
- sandbox/resource setup failure;
- cleanup warning/failure.

Cancellation/timeout must target descendants, not only the direct child.

## 6. Required tests

- echo/stdout/stderr;
- nonzero exit;
- stdin bytes and null stdin;
- environment allow/deny;
- cwd;
- huge output with bounded capture;
- slow streaming receiver;
- terminate-on-overflow;
- cancellation before spawn;
- cancellation while running;
- timeout;
- cancel vs natural-exit race;
- descendant/grandchild cleanup;
- spawn failure;
- cleanup diagnostics;
- platform-specific process-tree fixture.

## 7. Required verification

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
```

Run platform-hosted lifecycle tests on each platform claimed qualified; cross-compilation alone is not enough for descendant cleanup.

## 8. Acceptance criteria

1. One production runner path owns finite process spawn.
2. No shell fallback exists.
3. Output retention is bounded while pipes remain drained.
4. Timeout/cancel terminate process descendants.
5. Terminal classification and cleanup evidence are machine-readable.
6. No scheduler/network semantics enter runner API.
7. Focused and broad tests pass.

## 9. Stop conditions

Stop if:

- platform cleanup would require pretending direct-child kill is tree cleanup;
- sandbox/resource hook design would make best-effort look enforced;
- implementation starts duplicating a server-side admission queue.

## 10. Closure evidence

Create `plans/closure/foundation-execution-core/002-status.md` with process-lifecycle requirement/evidence matrix and actual platform qualification.
