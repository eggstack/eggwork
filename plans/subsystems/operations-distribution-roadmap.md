# Operations and Distribution Roadmap

Status: active roadmap

Canonical authority:

- `plans/000-long-term-specification.md#21-platform-and-packaging-targets`
- `plans/000-long-term-specification.md#22-deferred-capabilities`
- `plans/002-long-term-roadmap.md#phase-6--operations-packaging-and-node-administration`
- `plans/002-long-term-roadmap.md#phase-9--reverse-connect-and-interactive-extensions`

## 1. Ownership boundary

This subsystem owns configuration, node identity/config administration UX, status/doctor/drain commands, service lifecycle, logs/metrics/operator diagnostics, packaging/release assets, Eggup integration, storage inspection/GC commands, reverse-connect transport broker, and later interactive PTY client/daemon surfaces.

It does not own process lifecycle internals, scheduler placement, or CodeGG semantics.

## 2. Durable invariants

1. Installation/update/service actions have explicit ownership.
2. Update cannot fabricate execution completion.
3. Drain blocks new acceptance without becoming a queue.
4. Operator output is secret-safe.
5. Release artifacts are checksummed and provenance is inspectable.
6. Reverse relay is transport only.
7. PTY attachment ownership is authenticated and bounded.

## 3. Milestones

### M001 — Node operations surface

Class: capability/polish

Status: active; M001 implementation is complete and its closure evidence is being finalized

Implementation plan:

- `plans/implementation/operations-distribution/001-node-operations-surface.md`

Objective:

Add configuration, status, doctor, drain/undrain, execution/storage inspection, cleanup/GC commands, and stable machine-readable operator output.

### M002 — Packaging, services, and Eggup

Class: infrastructure/capability

Status: blocked on M001 and stable Eggup consumer interfaces

Implementation plan:

- `plans/implementation/operations-distribution/002-packaging-services-and-eggup.md`

Objective:

Ship prebuilt binaries for qualified targets, integrate verified install/update/service management through Eggup where available, and avoid copying lifecycle machinery.

### M003 — Metrics and operational qualification

Class: polish/invariant

Status: blocked on M002

Objective:

Add bounded metrics, resource/storage diagnostics, service restart/upgrade fixtures, and hosted release evidence.

### M004 — Reverse-connect relay

Class: capability

Status: deferred; blocked on stable Control Plane M002 semantics

Objective:

Permit nodes behind NAT/firewalls to maintain an authenticated outbound link through a dumb relay while preserving the same fixed-target execution identities.

Non-goal: any worker selection, global queue, or retry routing in the relay.

### M005 — Interactive PTY extension

Class: capability

Status: deferred; blocked on stable leases/event resume and explicit platform PTY design

Objective:

Add create/attach/detach/input/resize/terminate/resume with bounded scrollback and transport-owned attachments. This must not change noninteractive runner ownership.

## 4. Target matrix

Initial intended binary targets:

- x86_64-unknown-linux-gnu;
- aarch64-unknown-linux-gnu;
- x86_64-apple-darwin;
- aarch64-apple-darwin;
- x86_64-pc-windows-msvc.

Additional musl/ARM variants are qualification work, not assumed support.

## 5. Verification strategy

- clean-install smoke;
- exact-version update/rollback where supported;
- service manager ownership;
- running/stopped/draining update cases;
- upgrade during retained terminal records;
- restart reconciliation;
- checksum/provenance;
- cross-platform hosted CI;
- reverse relay disconnect/backpressure when implemented;
- PTY ownership/resync/adversarial input when implemented.
