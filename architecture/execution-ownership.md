# Execution ownership

`eggwork-runner` is the canonical owner of finite-process lifecycle and resource-wrapper invocations. `eggwork-sandbox-helper` may create the target process only to apply installation-owned sandbox/resource mechanics before launch. `eggwork-server`, `eggwork-client`, and `eggwork-core` must not create OS children. Blocking database/filesystem work with `spawn_blocking` is not child-process ownership.

`LocalProcessRunner` accepts argv, executes inside a caller-authorized root, clears and rebuilds the environment from a conservative baseline, writes bounded stdin, and concurrently drains stdout and stderr. Capture retains bounded head/tail bytes; a full event channel drops stream chunks while pipe draining continues. Timeout, cancellation, terminate-on-overflow, and leader exit with remaining descendants signal the process group and force-kill after a grace period. Cleanup diagnostics do not rewrite a known exit outcome.

Run `python3 scripts/check_execution_ownership.py` before submitting Rust changes. It scans production source files, checks crate dependency direction, and runs a synthetic forbidden-spawn self-test. Approved owners are explicit in the script. A future platform backend must document its ownership rationale and add a negative/positive fixture before its crate or source path is allowlisted. `RunnerRequest` is constructed through `RunnerRequest::from_spec`; its validated execution fields are intentionally private.

On non-Unix hosts the runner returns an explicit unsupported-platform error until a process-tree backend is implemented and host-qualified. Linux was exercised for this milestone. The server performs immediate local admission in a later plan and retains its permit until runner cleanup completes. It does not maintain a queue or select another node. The caller retains global scheduling, placement, and retry authority.

These boundaries follow [ADR-0005](../plans/adrs/ADR-0005-runner-service-separation-and-local-admission.md) and the [foundation roadmap](../plans/subsystems/foundation-execution-core-roadmap.md). Network listeners remain deferred to later milestones.
