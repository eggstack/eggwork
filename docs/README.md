# Eggwork documentation

Operator and integrator documentation. `README.md` at the repository root is the
short quickstart; this directory holds the detail it links to.

| Document | Read it when |
|---|---|
| [quickstart.md](quickstart.md) | Building a node from source and proving it executes. Start here. |
| [operations.md](operations.md) | You operate a node: the full `eggworkd` verb reference, drain, GC, inspection |
| [deployment.md](deployment.md) | You install or update a node as service software via Eggup |
| [releases.md](releases.md) | You consume a published release, or you cut one |

## Design documents are elsewhere

This directory is **user- and operator-facing**. The internal design record is
not duplicated here:

- [`architecture/`](../architecture/overview.md) — module map, request flow, and
  the **divergences table** of known open questions. Start at
  `architecture/overview.md`.
- [`plans/`](../plans/registry.md) — the planning system; `registry.md` is the
  active control surface.
- [`CONTRIBUTING.md`](../CONTRIBUTING.md) — the pre-submit gate.
- [`.skills/`](../.skills/README.md) — task-shaped agent guidance.

If a document here and a deep dive in `architecture/` disagree, the deep dive
wins.
