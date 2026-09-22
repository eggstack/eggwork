# Foundation M002 Follow-up — Control Plane Dependency Blocker

This addendum preserves the historical Foundation M002 closure at `002-status.md` and updates its downstream readiness finding after a required dependency-resolution check.

## Finding

Control Plane M001 cannot begin implementation against the public dependency surface. The reviewed local EggServe checkout at `7e52ecec0b336ef1755b3e1654ddb1438b38bcdb` contains the required EggServe 0.2 peer-authenticated request context and Eggnet TLS client-auth APIs, but Cargo resolution cannot find the required EggServe 0.2 crates (`eggserve-core`, `eggserve-primitives`, `eggserve-server`, and `eggnet-tls`) in crates.io. In particular, resolution for `eggserve-core = "^0.2.0"` offered only `0.1.2`, `0.1.1`, and `0.1.0`. The matching APIs therefore are not available to Eggwork as a published dependency.

The implementation was not redirected to the sibling checkout, a Git pin, another HTTP stack, or copied transport code. Those options would bypass the plan's required public EggStack dependency boundary and would not produce a releasable Eggwork build.

## Evidence

- `cargo check --workspace` after adding the required dependency failed during resolution with: `failed to select a version for the requirement eggserve-core = "^0.2.0"`; available candidates were `0.1.2`, `0.1.1`, and `0.1.0`.
- `cargo info eggserve-core@0.2.0`, `eggnet-tls@0.2.0`, `eggserve-primitives@0.2.0`, and `eggserve-server@0.2.0` each reported that the version could not be found in the crates.io index.
- `cargo info eggfetch-core@0.2.0` succeeded; Eggfetch 0.2.0 and its needed APIs are published.
- Local EggServe checkout was clean at `7e52ecec0b336ef1755b3e1654ddb1438b38bcdb` and declares workspace version `0.2.0`.
- EggServe's verified certificate state is available through `Request::context().connection().tls`; required client authentication is supported by Eggnet TLS. These APIs were inspected but cannot yet be consumed from the public registry release.
- Incomplete Control Plane code/dependency edits were reverted. No control-plane implementation is claimed.

## Required upstream prerequisite

Publish the matching EggServe 0.2 crate set (`eggserve-core`, `eggserve-primitives`, `eggserve-server`, and `eggnet-tls`) with the reviewed peer-authentication context, request lifecycle, and streaming APIs, then re-run dependency/API checks against those published releases.

## Disposition

Control Plane M001 is **blocked**. Later plans in the requested sequence remain blocked by dependency order. Resume at Control Plane M001 after the upstream release exists; re-check the public API and Cargo resolution before coding.
