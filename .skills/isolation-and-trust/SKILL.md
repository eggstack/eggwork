---
name: isolation-and-trust
description: Work on eggwork's Landlock/cgroup isolation, helper trust boundary, or capability advertisement. Use when touching verify_trusted_helper, the sandbox helper, resource enforcement, admission, or any code that claims or enforces isolation. Explains the single trust function, fail-closed vs NotApplied, and the five release targets' build constraints.
---

# Isolation and trust

Eggwork advertises isolation only when it can enforce it, and the enforcement
path and the advertisement path are the **same function** so they cannot disagree.

## The single trust boundary

`eggwork_runner::verify_trusted_helper(path) -> Result<(), String>` is the whole
Linux helper trust decision. Every caller delegates to it:

- capability advertisement (what the node claims it can do),
- `doctor` (what the operator is told),
- admission (what an execution is allowed to require).

A helper is trusted when it is root- or effective-user-owned, not group/world
writable, and inside non-writable ancestor directories. **Anything else fails
closed** — required isolation stays unsupported rather than degrading.

If you add a caller, call `verify_trusted_helper`. Do not re-implement the
checks, and do not add a second policy copy. A node that claims isolation its
enforcement path would refuse is the exact bug this design exists to prevent.

There is a non-Linux stub at the same path that always errors, so the
`#[cfg]`-gated machinery still builds on all five release targets.

Note the deliberate asymmetry: the **sandbox helper** is user-scope-installable
and checked by `verify_trusted_helper`, but `systemd-run`/`systemctl` are *not*
— the runner applies its own stricter root-only trust pattern to those binaries
in `SystemdCgroupBackend::discover()`/`systemctl_path()`. Two boundaries, two
rules, both documented in `architecture/runner-execution.md`.

## Fail closed vs degrade

This distinction is the product's defining safety property. Get it backwards and
you either break working executions or silently under-isolate.

| Situation | Outcome |
|---|---|
| `Required` sandbox profile cannot be enforced | Kill the helper, fail the execution with `RunnerError::RequiredSetupUnavailable` |
| `Required` resource dimension cannot be enforced | Fail the execution with a setup error |
| *Optional* / *best-effort* requirement cannot be applied | Run unsandboxed, record `NotApplied` **with a reason** |
| Untrusted helper | Fail closed — never advertise, never admit a required profile |
| Version skew between daemon and helper | Fail closed |
| Missing prior-generation digest on update | Fail closed for existing files |
| Unsupported service backend | Refuse before any mutation |

Degradation happens **only** where the caller never demanded the guarantee, and
it is always recorded. A silent `NotApplied` with no reason is a bug.

## Windows and non-Linux

Windows service management is **unsupported and fails closed**. Hosted
qualification registered the `eggwork-node` SCM service and then failed to start
it with error 1053, because `eggworkd` implements no service-control dispatcher —
the process the SCM launches never connects back to the manager. Registering an
external process would produce a working installation with *no service behind
it*, which is worse than an honest refusal. So every mutating `service` verb on
Windows is refused before any mutation. The typed adapter is retained in
`deployment::windows_scm_manager` (`#[cfg(windows)]` + `#[allow(dead_code)]`) for
the future service-host milestone (Operations M007, status **ready**).

`--windows-start-type` appears in `scripts/qualify_release.py` and the README,
but **no code parses it**: the Windows arm discards `args` and refuses before
reaching a manager. Harmless today; it becomes live work when M007 lands. Do
not treat the flag as a supported knob.

On non-Unix hosts the runner returns an explicit unsupported-platform error
before any child exists. This is also the actual cause of the hosted-Windows
`Internal`/`null` execution failure recorded in
`architecture/release-qualification.md` — an earlier output-monitor race was
blamed for it and that attribution was **wrong**. The lesson recorded there
applies to your own debugging: a hypothesis that explains a symptom is not
evidence.

## Linux-only machinery

`#[cfg]`-gate Landlock and cgroups with an **explicit fail-closed stub**, never a
silent no-op, so all five release targets build:

- `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` — daemon + Landlock
  helper shipped as one bundle from one source revision, so the pair cannot drift.
- `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc` —
  daemon only.

`landlock` is Linux-only; declaring it unconditionally breaks the non-Linux
builds (a real hosted-run failure). Never set `RUSTFLAGS` — it replaces the
target-scoped Windows link policy in `.cargo/config.toml` that makes MSVC
release binaries reproducible.

## Reference

- [architecture/sandbox-helper.md](../../architecture/sandbox-helper.md) — Landlock, cgroup verification, supervision
- [architecture/runner-execution.md](../../architecture/runner-execution.md) — setup seam, systemd wrapping, `NotApplied` recording
- [architecture/server-node.md](../../architecture/server-node.md) — admission
- [architecture/deployment-lifecycle.md](../../architecture/deployment-lifecycle.md) — Windows fail-closed path
- [AGENTS.md](../../AGENTS.md) — invariants 4 and 5
