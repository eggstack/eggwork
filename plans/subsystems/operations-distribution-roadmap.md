# Operations and Distribution Roadmap

Status: active roadmap; M002 blocked on Eggup post-commit rollback contract, producer packaging waits on Eggpack

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

Status: closed

Closure evidence: `plans/closure/operations-distribution/001-status.md`

Implementation plan:

- `plans/implementation/operations-distribution/001-node-operations-surface.md`

Objective:

Add configuration, status, doctor, drain/undrain, execution/storage inspection, cleanup/GC commands, and stable machine-readable operator output.

### M002 — Eggup deployment and service integration

Class: infrastructure/capability

Status: blocked on Eggup post-commit rollback contract

Implementation plan:

- `plans/implementation/operations-distribution/002-eggup-deployment-and-service-integration.md`

Supersedes:

- `plans/implementation/operations-distribution/002-packaging-services-and-eggup.md`

Objective:

Use Eggup for consumer-side verified multi-artifact deployment, rollback, install ownership, and native service lifecycle while keeping Eggwork's drain/restart policy application-owned.

Current interface review confirms Eggup main contains Unix manager mechanics and a native Windows SCM adapter. However, its transaction API does not retain backups or expose post-commit rollback for the required post-start health failure case. M002 remains blocked until Eggup provides that contract or the architecture owner revises the rollback ownership requirement. Evidence: `plans/closure/operations-distribution/002-status.md`.

### M003 — Eggpack producer packaging integration

Class: infrastructure/capability

Status: blocked on Eggpack ReleaseManifest and build/qualification interfaces

Objective:

Map Eggwork's release target/artifact policy into Eggpack's producer-side contracts, manifests, build/qualification plan, bootstrap installer, and generated release-CI surfaces without recreating producer distribution logic in Eggwork.

Do not write the implementation handoff until Eggpack closes the concrete ReleaseManifest and build/qualification interfaces needed by a consumer repository.

### M004 — Operational and release qualification

Class: polish/invariant

Status: blocked on M002 and M003

Objective:

Run hosted target/service/update/recovery qualification, verify checksums/provenance and installed-version coherence, exercise drain/update/restart boundaries, and close release-operational findings.

### M005 — Reverse-connect relay

Class: capability

Status: deferred; stable identity/lease/event semantics are available, but no immediate handoff is authorized

Objective:

Permit nodes behind NAT/firewalls to maintain an authenticated outbound link through a dumb relay while preserving the same fixed-target execution identities.

Non-goal: any worker selection, global queue, or retry routing in the relay.

### M006 — Interactive PTY extension

Class: capability

Status: deferred; requires a separate PTY ownership/attach design

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
