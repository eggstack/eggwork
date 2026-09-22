# Eggwork Subsystem Roadmaps

Subsystem roadmaps translate the canonical specification and ADRs into coherent dependency-ordered workstreams.

## Active subsystem roadmaps

- `foundation-execution-core-roadmap.md` — workspace bootstrap, core domain types, and canonical local runner.
- `control-plane-protocol-roadmap.md` — authenticated fixed-target server/client, execution registry, leases, events, and restart semantics.
- `workspace-artifact-transport-roadmap.md` — blob store, manifests, materialization, declared outputs, artifacts, quotas, and GC.
- `security-isolation-resource-roadmap.md` — authorization boundaries, sandboxing, resource enforcement, redaction, and negative testing.
- `operations-distribution-roadmap.md` — configuration, node administration, packaging, updater/service integration, reverse-connect, and future interactive transport.
- `codegg-integration-roadmap.md` — CodeGG executor integration and later remote AgentRun worker mode.

## Roadmap rules

Each roadmap must identify:

- subsystem ownership boundary;
- durable invariants;
- milestone classes: invariant, capability, infrastructure, or polish;
- dependencies;
- exit criteria;
- cross-cutting tests;
- deferred work.

Roadmaps must not duplicate the master roadmap verbatim. They add subsystem detail and execution ordering.
