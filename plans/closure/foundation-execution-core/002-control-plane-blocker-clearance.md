# Foundation M002 Follow-up — Control Plane Dependency Clearance

This note supersedes the current-readiness finding in `002-control-plane-blocker-addendum.md`. It does not alter the historical record of why Control Plane M001 was blocked at commit `0db3bd7`.

## Evidence

On the resumed run, crates.io metadata lookup succeeded for:

- `eggserve-core 0.2.0`;
- `eggserve-primitives 0.2.0`;
- `eggserve-server 0.2.0`;
- `eggnet-tls 0.2.0`;
- `eggfetch-core 0.2.0`.

The published EggServe API metadata exposes the `tls` feature on `eggserve-core`; Eggfetch 0.2.0 exposes the lean `standard-http1` and `tls-rustls` features. Source/API verification and compilation against these registry releases remain part of Control Plane M001 and its closure evidence.

## Disposition

The publication blocker is cleared. Control Plane M001 is active. If Cargo resolution or public API compilation fails, stop and add a new blocker record rather than switching to unpublished sibling paths or copied transport implementations.
