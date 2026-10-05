---
name: architecture-routing
description: Index into eggwork's architecture/ deep dives - which file answers which question, and which code owns which boundary. Use before reading source, when you need to know where a behavior is implemented or which doc is normative, or when deciding whether a change is a bug or a doc fix.
---

# Architecture routing

`architecture/overview.md` is the bird's-eye view. The rest are per-component
deep dives, each written to be read *instead of* the source when you need to
understand a boundary. Read the index; do not grep the whole tree.

## Which doc answers which question

| Question | Read |
|---|---|
| What is the product, and what is it deliberately not? | [README](../../README.md), [overview.md](../../architecture/overview.md) |
| How do I know what a term means? | [domain.md](../../architecture/domain.md) |
| What is decided vs still open at the wire level? | [protocol.md](../../architecture/protocol.md) |
| Who is allowed to create a process? | [execution-ownership.md](../../architecture/execution-ownership.md) |
| Request/result contracts, identities, limits, digests | [core-domain.md](../../architecture/core-domain.md) |
| Process launch, env rebuild, cancellation, setup seam | [runner-execution.md](../../architecture/runner-execution.md) |
| Landlock, cgroup verification, helper supervision | [sandbox-helper.md](../../architecture/sandbox-helper.md) |
| mTLS, retries, egress routing, the lease-in-URL issue | [client-transport.md](../../architecture/client-transport.md) |
| Admission, authorization, leases, execution lifecycle | [server-node.md](../../architecture/server-node.md) |
| SQLite journal, state machine, pagination | [storage-journal.md](../../architecture/storage-journal.md) |
| Blobs, workspace materialization, artifacts, GC | [data-plane.md](../../architecture/data-plane.md) |
| Operator verbs, `doctor`, drain, GC preview | [operations-cli.md](../../architecture/operations-cli.md) |
| Eggup install/update/rollback, service policy | [deployment-lifecycle.md](../../architecture/deployment-lifecycle.md) |
| Producer vs consumer authority | [distribution.md](../../architecture/distribution.md) |
| Release harness, hosted qualification, failure history | [release-qualification.md](../../architecture/release-qualification.md) |
| CI jobs, ownership guard, `--locked` discipline | [ci-guardrails.md](../../architecture/ci-guardrails.md) |

Planning is separate from architecture: `plans/registry.md` is the active
control surface, and `plans/README.md` explains the document hierarchy.

## Code that owns each boundary

| Boundary | Owner | Notes |
|---|---|---|
| OS process lifecycle | `eggwork-runner` | The only crate allowed to spawn, alongside the helper |
| Target confinement | `eggwork-sandbox-helper` | Standalone binary, no workspace deps; spawns to confine, then supervises |
| Domain vocabulary | `eggwork-core` | Protocol-neutral; no internal deps |
| Node, admission, stores, ops | `eggwork-server` | Must never spawn a child process |
| Fixed-target client | `eggwork-client` | Explicit node id; `eggress-route` feature must not change target identity |

Dependency direction is one-way and machine-checked: core/helper → nothing,
client → core, server → core + runner. The server's `eggwork-client` edge is a
`dev-dependency` for the qualification harness.

## The two facts that most often get misread

**The helper does not `execve`.** It `spawn`s the target and `wait`s on it, so it
can read post-mortem cgroup counters and report the real resource outcome back to
the runner over a `UnixStream` status channel. Confinement is unaffected — the
Landlock ruleset is inherited across the fork and survives the target's own
`execve`. This is the one documented exception to singular process ownership.

**Degradation is scoped to guarantees the caller did not demand.** A `Required`
sandbox profile or resource dimension that cannot be enforced kills the helper
and fails the execution with `RequiredSetupUnavailable`. Only an *optional* or
*best-effort* requirement runs unsandboxed, and it records `NotApplied` with a
reason. Degradation is always reported, never silent.

## `overview.md` has a divergences table — read it

The "Divergences and open questions" section lists cross-cutting places where
code, docs, and stated invariants do not line up: implementation-vs-prose
mismatches, invariants weaker in practice than their wording, store
correctness questions, and client transport hygiene. Each links to the deep
dive that established it.

**Check that table before diagnosing a bug.** Several entries describe known,
deliberate, *documented* state. If you "discover" one of them fresh, it is already
recorded — your job is to decide whether the code, the doc, or the invariant
should move, not to treat it as a novel finding. Two examples that are accurate
today: `blob_download` takes no reference lease (safe only because Unix keeps an
open descriptor alive), and the deployment helper check is the weaker subset of
`verify_trusted_helper` (file only, no ancestor walk).

## Before you call something a bug

Ask which of these it is, because they have very different responses:

1. **Code contradicts a deep dive** — the deep dives are written to be accurate.
   Check the deep dive first; if it agrees with the code, the prose at page level
   (`overview.md`, `AGENTS.md`, `README.md`) is what drifted.
2. **A known documented divergence** — already in the table. Not novel.
3. **A real defect** — write it up with a `file:line` citation and a concrete
   failure, then check `plans/registry.md` before opening work, since a plan may
   already name it.

Prefer fixing a drifting invariant statement over deleting a real guard. An
absolute-sounding invariant that the code legitimately violates is worse than no
invariant: a future agent auditing against it reaches a confidently wrong
conclusion. `AGENTS.md` invariant 2 is a good example of the failure mode, and
the fix is to narrow the wording, not to remove the rule.
