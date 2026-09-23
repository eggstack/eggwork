# eggwork

Eggwork is a Rust-native, fixed-target remote execution fabric: a caller selects one node, submits a bounded execution request, observes machine-readable lifecycle events, and receives terminal state plus declared artifacts.

Eggwork is deliberately **not another scheduler**. Global queues, worker selection, priorities/fairness, workflow DAGs, and semantic retry remain caller-owned. This makes Eggwork suitable as an execution backend for systems such as CodeGG without competing with their orchestration policy.

The protocol-neutral core, canonical local finite-process runner, authenticated remote control plane, durable leases, workspace transfer, declared artifact capture, bounded retention/garbage collection, and the authorization/redaction foundation are implemented. Host sandbox enforcement and resource controls, the node operations surface, and the CodeGG adapter remain planned work.

Start here:

- `plans/README.md` — planning system and document hierarchy
- `plans/000-long-term-specification.md` — canonical product/architecture specification
- `plans/002-long-term-roadmap.md` — dependency-ordered long-term roadmap
- `plans/registry.md` — current ready/blocked work and external interface baselines
- `plans/closure/` — implementation and verification evidence

The next implementation handoff is listed in the registry.
