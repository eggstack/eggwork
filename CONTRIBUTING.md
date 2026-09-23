# Contributing

Changes must preserve the scheduler-free fixed-target boundary and crate ownership described in `architecture/`. Run formatting, Clippy, workspace tests, workspace check, and `python3 scripts/check_execution_ownership.py` before submitting Rust changes. Production process creation belongs only to `eggwork-runner`, with the documented sandbox-helper exception. Planning changes must follow `plans/003-planning-process.md`.
