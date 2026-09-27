# eggwork

Eggwork is a Rust-native, fixed-target remote execution fabric: a caller selects one node, submits a bounded execution request, observes machine-readable lifecycle events, and receives terminal state plus declared artifacts.

Eggwork is deliberately **not another scheduler**. Global queues, worker selection, priorities/fairness, workflow DAGs, and semantic retry remain caller-owned. This makes Eggwork suitable as an execution backend for systems such as CodeGG without competing with their orchestration policy.

The protocol-neutral core, canonical local finite-process runner, authenticated remote control plane, durable leases, workspace transfer, declared artifact capture, bounded retention/garbage collection, authorization/redaction, Linux sandbox/resource controls, the first node operations surface, and Eggup-backed consumer deployment/service lifecycle are implemented. The CodeGG adapter remains planned work.

## Node operations

`eggworkd` provides a JSON-only operator interface. It reads a versioned JSON config, requires a verified mTLS client CA and explicit per-principal operation grants, and redacts the private-key path and client certificate fingerprints from `config print`.

See [examples/eggworkd.example.json](examples/eggworkd.example.json) for the config shape. Replace its example paths and certificate fingerprint with installation-specific values; keep the private key owned by the node account and readable only by that account.

```text
eggworkd config validate --config /etc/eggwork/node.json
eggworkd doctor --config /etc/eggwork/node.json
eggworkd run --config /etc/eggwork/node.json
eggworkd status --config /etc/eggwork/node.json
eggworkd drain --config /etc/eggwork/node.json
eggworkd undrain --config /etc/eggwork/node.json
eggworkd executions list --config /etc/eggwork/node.json --limit 50 --offset 0
eggworkd executions show --config /etc/eggwork/node.json <execution-id>
eggworkd storage summary --config /etc/eggwork/node.json
eggworkd inspect blob|workspace|artifact <id> --config /etc/eggwork/node.json
eggworkd gc --config /etc/eggwork/node.json --limit 128       # read-only preview
eggworkd gc --config /etc/eggwork/node.json --apply --limit 128
eggworkd deployment status --config /etc/eggwork/node.json
eggworkd service spec --config /etc/eggwork/node.json --executable /opt/eggwork/bin/eggworkd --service-config /etc/eggwork/node.json
eggworkd service status --config /etc/eggwork/node.json --executable /opt/eggwork/bin/eggworkd --service-config /etc/eggwork/node.json
eggworkd service install|start|stop|restart|uninstall --config /etc/eggwork/node.json --executable /opt/eggwork/bin/eggworkd --service-config /etc/eggwork/node.json --unit-path /etc/systemd/system/eggwork-node.service --scope system
eggworkd version
```

Consumer deployment and service lifecycle go through Eggup (`eggup-core`/`eggup-service` 0.1.1): the install unit is `bin/eggworkd` plus `bin/eggwork-sandbox-helper` moved in one transaction, service ownership is proven by exact executable plus critical argv/config before any stop/replace/uninstall, and a bounded post-update health probe runs while Eggup retains backups (`RollBack` is the default). Updates drain first: set persistent drain with `eggworkd drain`, wait for active executions (or pass explicit force policy through the orchestration API), then replace. Service lifecycle mutation is qualified on Linux systemd; other platforms receive a structured refusal until hosted evidence exists. Producer packaging (release manifests, installers, release CI) remains owned by Eggpack and is out of scope here.

Drain is stored beside the execution database and is observed by a running node before it accepts each new execution. Local GC previews are read-only; applying GC is bounded and takes an exclusive state lock, so it fails while the node process is running. A live service can use `NodeServer::collect_garbage`, which operates on its existing stores and preserves active references.

Start here:

- `plans/README.md` — planning system and document hierarchy
- `plans/000-long-term-specification.md` — canonical product/architecture specification
- `plans/002-long-term-roadmap.md` — dependency-ordered long-term roadmap
- `plans/registry.md` — current ready/blocked work and external interface baselines
- `plans/closure/` — implementation and verification evidence

The next implementation handoff is listed in the registry.
