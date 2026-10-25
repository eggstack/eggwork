# Operations M008a — Operator surface truthfulness corrective

Status: closed

Closure record: `plans/closure/operations-distribution/008a-status.md`

Class: capability

Source authority:

- `plans/subsystems/operations-distribution-operator-truthfulness-corrective-addendum.md` — findings OP-1, OP-2, OP-3
- `plans/subsystems/operations-distribution-roadmap.md` — the operations surface milestones and their durable invariants
- `plans/closure/operations-distribution/001-status.md` — the node operations surface
- `architecture/operations-cli.md` — the operator command surface, its output discipline, and its redaction property
- `architecture/server-node.md` — admission, leases, and lifecycle
- `architecture/core-domain.md` — the wire types the client deserializes
- `plans/003-planning-process.md` — status vocabulary, milestone sizing, dependency discipline, required plan sections, closure evidence, corrective work, architecture changes

## 1. Objective

Make the node's operator surface report what the node actually knows, name each fact exactly once, and say what is wrong when the configuration is wrong.

Three concrete defects are in scope:

1. five metrics the server increments are never reported by `eggworkd metrics` / `eggworkd status`, because the operator surface keeps a hand-maintained name list instead of reading the producers;
2. two different facts are both called `active_executions` — permits held, and non-terminal execution rows — in the API status response and in the operator's own listing;
3. every configuration fault collapses into `configuration is invalid or unreadable`, so an operator with one wrong file mode is told nothing about which rule failed.

After M008a, every metric the server can increment is visible to the operator; the two `active_executions` facts have distinct names and documented meanings; and a configuration failure names the rule it violated and the role of the offending item, without disclosing a private-key path or a certificate fingerprint.

## 2. Baseline / problem statement

At the reviewed baseline (`81f34e0`):

**OP-1 — the metric vocabulary has no owner that is the producer.**

- `crates/eggwork-server/src/operations.rs` declares `const METRIC_NAMES: &[&str]`, a hand-maintained list of metric names.
- `crates/eggwork-server/src/operations.rs` → `fn metrics_snapshot` seeds its counter map from `METRIC_NAMES`, then reads rows from the node metrics table and copies a value **only if the name is already in the map**. A metric absent from the list is silently discarded.
- `crates/eggwork-server/src/lib.rs` increments five metrics that are not in that list: `workspace_manifest_registrations`, `workspace_derived_requests`, `workspace_derived_hits`, `workspace_patch_entries`, `workspace_base_missing`. The workspace-derivation path increments three of them in one call.
- There is no test comparing `METRIC_NAMES` against the names the server can increment, and no test asserting that a freshly incremented metric appears in a snapshot.

**OP-2 — one name, two facts.**

- `crates/eggwork-server/src/lib.rs` → `fn node_status` computes `active_executions` as `max_active` minus available permits — *permits currently held*. The field is `eggwork_core::NodeStatus::active_executions`, a serialized wire type read by `eggwork-client` → `NodeClient::status`.
- `crates/eggwork-server/src/operations.rs` computes `active_executions` for the operator's execution listing as *the number of database rows in a non-terminal state*.
- `crates/eggwork-server/src/bin/eggworkd.rs` writes the key `active_executions` into the operator's status payload.
- The two facts legitimately differ: an execution that has crashed but has not been recovered still has a non-terminal row while its permit has been released. There is no field name that lets an operator tell which number is which.

**OP-3 — the configuration fault kind is discarded.**

- `crates/eggwork-server/src/operations.rs` returns `OperationsError::Config` at 25 sites covering every distinct configuration fault: unreadable file, wrong permissions, wrong owner, a sandbox helper outside a world-writable ancestor, a malformed field, an unparsable bind address, a failed serialization of the redacted config, and more.
- `crates/eggwork-server/src/bin/eggworkd.rs` maps the load failure to the single string `configuration is invalid or unreadable`.
- `AGENTS.md` records this as a known operator-visible pain point: the config loader's strictness "is undocumented anywhere else and produces a single unhelpful `configuration is invalid or unreadable`".
- `architecture/operations-cli.md` records the countervailing property that must survive: the binary "never interpolates an error's source text for library errors, no filesystem path, key detail, or child-process output can reach stderr through that path". The fix must add actionable detail without becoming a leak.

## 3. Scope

In scope:

- the metric vocabulary and the report path in `crates/eggwork-server/src/operations.rs`;
- the metric increment sites in `crates/eggwork-server/src/lib.rs`;
- the operator-visible name and documentation of the two `active_executions` facts, in `crates/eggwork-server/src/operations.rs` and `crates/eggwork-server/src/bin/eggworkd.rs`;
- the configuration error taxonomy in `crates/eggwork-server/src/operations.rs` and its rendering in `crates/eggwork-server/src/bin/eggworkd.rs`;
- the `config print` and `config validate` verbs' reporting of configuration faults;
- the corresponding operator documentation in `docs/operations.md` and `docs/quickstart.md`, and the design record in `architecture/operations-cli.md`;
- tests for each of the above.

Out of scope:

- `crates/eggwork-core/src/lib.rs` → `NodeStatus` field names and the wire shape of the status response. This plan does **not** rename a wire field (see §9).
- The `doctor` check list and its ordering, except where a configuration fault is reported through it.
- The blob-inspection verb's re-derivation of the blob store's shard path layout. That is a further instance of the same defect class, observed during the interrogation pass, and it is **not** in this milestone; it is recorded here so it is not lost, and it is a candidate for a later corrective.
- The service-host lifecycle, the shutdown-and-wait convergence contract, and the Windows service path. Those belong to M007.
- The release harness, the design record's citation hygiene, and the CI feature-gate gap. Those belong to M008b.
- Any change to admission, authorization, lease, generation, fencing, or digest behaviour.

## 4. Invariants and non-goals

Invariants this plan must not weaken:

- The scheduler-free, fixed-target boundary. `eggworkd` remains a single-node operator surface; nothing in this plan introduces a queue, worker selection, priority, or DAG.
- Authentication and authorization. The operator verbs keep their existing access model; no fault message may disclose the contents of an authorization decision, a principal, or a trust store.
- Lease, generation, and fencing bounds. The permit accounting that `fn node_status` reports is the same accounting that gates admission; this plan changes the *name* of one reported fact, never the accounting.
- Digest bounds. No artifact, workspace, or blob digest semantics change.
- The redaction property in `architecture/operations-cli.md`: `config print` redacts private-key paths and certificate fingerprints, and a configuration *failure* message must not disclose them either.
- `unsafe_code = "deny"` and the workspace clippy policy; no new dependency may be introduced into `eggwork-server` for this work.

Non-goals:

- Making the metrics subsystem support arbitrary user-defined metric names. The vocabulary is closed and node-internal.
- Changing the metric *storage* format. The node metrics table's shape is not in scope; only the report path is.
- Turning `eggworkd` into a monitoring product with per-metric thresholds, alerts, or history.
- Rewriting the config loader's rules. The rules are correct and strict; the defect is that the fault kind is discarded on the way to the operator.
- Any user-facing or wire-visible capability beyond the three named defects.

## 5. Required production changes

### 5.1 OP-1 — one owner for the metric vocabulary

The producer must own the vocabulary. Two acceptable shapes, in order of preference:

- **Preferred (derived):** the set of metric names the server can increment is derived from the producers rather than restated. This requires the increment sites to be the single registration point — for example a typed `Metric` enum or a `const`-declared set of names that `increment_metric`/`increment_metrics` accept, so that incrementing an unregistered name is a compile error rather than a runtime drop. If the names are strings in the database layer, the Rust-side vocabulary must still be the only list, and the report path must read *that* list rather than a second one.
- **Acceptable minimum (checked):** `METRIC_NAMES` remains hand-maintained, but a test derives the set of names the server can increment and fails when the two sets differ.

In either shape:

- `fn metrics_snapshot` must report every metric present in the node metrics table, and must not silently discard a row whose name is not in the vocabulary. A row whose name is unknown to the vocabulary must be either reported under its own name or reported as an explicit `unknown` bucket — never dropped without trace.
- The five currently invisible metrics — `workspace_manifest_registrations`, `workspace_derived_requests`, `workspace_derived_hits`, `workspace_patch_entries`, `workspace_base_missing` — must appear in `eggworkd metrics` and in the status payload.
- The workspace-derivation path's three-at-once increment must be covered by a test that asserts all three counters move.

### 5.2 OP-2 — two facts, two names, one wire field

The two facts must be separately named and separately documented.

**Decision: rename the operator-side field; leave the wire field alone.**

Rationale, stated explicitly because this is a protocol-visible choice:

- `eggwork_core::NodeStatus::active_executions` is a serialized wire type that deployed `eggwork-client` builds already deserialize. Renaming it removes a field those clients require, and a missing field is a deserialization error, not a defaulted value — so renaming it breaks every deployed client that reads node status.
- Adding a new field to `NodeStatus` is also a wire change (clients that use `deny_unknown_fields` would reject it; the current type does not, but that is not a guarantee the client may rely on across versions), so no new wire field is introduced either.
- The operator-side field is *not* a published protocol. `eggworkd`'s JSON output is operator-facing and versioned with the binary, not with a wire contract.

Therefore:

- The wire field `NodeStatus::active_executions` keeps its name and keeps its meaning (permits held). Its meaning must be stated in `architecture/core-domain.md` and in the `NodeStatus` doc comment, because the name alone does not distinguish it from the operator fact.
- The operator-side field must take a distinct name that says what it is. The plan requires the implementer to choose names that are self-describing and stable — for example one that says *permits held* and one that says *non-terminal executions* — and to record the chosen names in the closure record. The plan does not fix the literal strings, because the string is not the substance; the substance is that the two are distinguishable without reading prose.
- The operator surface must be able to show **both** facts. The API's permit count and the store's non-terminal count are different measurements, and an operator investigating a stuck execution needs both. The operator status output must carry both under their distinct names.
- The `eggworkd executions list` output must use the same distinct name for the non-terminal fact as the status output, so the two surfaces agree with each other.

### 5.3 OP-3 — actionable configuration failures

`OperationsError::Config` must be replaced by a typed taxonomy, or complemented by one, that preserves the fault kind and the rule.

Required shape of the operator-facing message:

- **which rule** was violated, stated as a stable, greppable rule identifier or a stable human phrase — for example a wrong file mode, a group- or world-writable path, a wrong owner, a path outside the installation root, a helper outside a world-writable ancestor, a malformed field, an unparsable bind address;
- **which file or path failed** — for every item whose path is not itself secret, the message names it: the config file, the state directory, the blob root, the workspace root, the sandbox helper path, the bind address. For a **secret-bearing** item — the private key, and any certificate-bearing item — the message names the *role* only ("the private key"), never the path, because naming that path is exactly the leak the redaction rule exists to prevent;
- **what the operator should do** — the corrective action in one sentence.

Requirements:

- The message must not contain a private-key path, a certificate fingerprint, a certificate subject, or any arbitrary underlying error text. Underlying `std::io::Error` / `serde_json` text is not interpolated; only the classified rule, the offending file or path, and the role of a secret-bearing item are.
- `config print` and `config validate` must surface the same detail for the same fault. `config print`'s existing redaction of private-key paths and certificate fingerprints is preserved unchanged — the redaction applies to the *values it prints*, and the new detail applies to *faults it reports*.
- `doctor`'s configuration check must report the same rule when it reports a configuration fault.
- The daemon's startup path must report the same detail; a node that refuses to start because of one wrong file mode must say so.
- The taxonomy must not become a channel for disclosing configuration *contents*. Reporting "the private key file must be mode 0600" is required; reporting the path to that file, its fingerprint, or its subject is not.

## 6. Failure, cancellation, restart, and contention semantics

- The operator surface is a read path. This plan adds no new failure mode to the execute path and changes no cancellation or restart behaviour.
- The metric report path must tolerate a metrics table containing a name the vocabulary does not know: it must report or explicitly bucket it, not error and not drop it. A node whose metrics table has been written by an older or newer binary must still serve `eggworkd metrics`.
- Reading the configuration must remain side-effect free. The new fault classification must not change which faults are fatal or which are warnings.
- A configuration fault is reported once, at the layer that detects it. The `eggworkd` CLI must not re-detect a fault in order to describe it; it must render the kind the loader produced.
- The permit count reported by `node_status` is sampled at response time and may differ from a concurrently-taken permit. This is existing behaviour and stays. The two facts in §5.2 are inherently independent samples; the plan must not add a lock that makes them artificially consistent, and the documentation must not claim they are equal.

## 7. Compatibility and migration

- **Wire protocol: unchanged.** `eggwork_core::NodeStatus` keeps its fields, including `active_executions`, with its existing meaning (permits held). No field is added, renamed, or removed. `eggwork-client` requires no change.
- **Operator output: changed.** The operator-side `active_executions` key is replaced by two distinctly named keys. This is a change to `eggworkd`'s JSON output, which is operator-facing and not a published protocol.
  - The plan requires the implementer to decide whether the old key is retained as a deprecated alias for one release, and to state that decision in the closure record. The default expectation is: retain the old key for one release, emitting it with the *store's* non-terminal meaning (the meaning the operator surface gave it before), and emit the new, unambiguous names alongside. If the implementer chooses not to offer an alias, the closure record must state why the operator-output break is acceptable and must not describe it as protocol-neutral.
  - The documentation in `docs/operations.md` must show the new names from the first release that carries them.
- **Metric output: additive.** Five metrics become visible. No existing metric changes meaning, disappears, or changes value. Any operator script that asserted "the workspace counters are zero" will now see non-zero values; that is the point of the fix, and the release notes must say so.
- **Configuration error text: changed.** Scripts that matched the literal string `configuration is invalid or unreadable` will no longer match. The closure record must list this and the release notes must carry it.
- **No released artifact format changes.** The `eggworkd` binary changes; the release's asset names, install identities, and target set do not.
- **Migration for operators:** the new configuration message is actionable as-is; no operator action is required beyond fixing what it names. The renamed operator field is documented in `docs/operations.md` and in the release notes for the first release that carries it.

## 8. Required tests

- **Metric coverage (fails when a producer increments an unreported metric).** A test that derives the set of metric names the server can increment and asserts it equals the set the operator surface reports. The test must be written so that it *fails* when a new increment site adds a name the surface does not report — this is the test the repository currently lacks, and it is the test that makes the drift detectable next time. A test that only asserts the current five are present is insufficient; it must be a set-equality or coverage check over the producers.
- **Metric visibility.** A test that increments each of the five currently invisible metrics through the real increment path and asserts each appears in the resulting `metrics_snapshot`.
- **Multi-name increment.** A test that exercises the workspace-derivation path's three-at-once increment and asserts all three counters move.
- **Unknown metric name.** A test that inserts a metrics row with a name outside the vocabulary and asserts the report path reports or explicitly buckets it rather than dropping it.
- **Distinct names.** A test asserting the operator status payload contains both facts under their distinct names, and that no key carries the name `active_executions` for the non-terminal fact.
- **Naming collision guard.** A test over the operator output schema asserting that no two keys in the same object carry the same name with different meanings — a small structural check that generalises the specific defect.
- **Wire field stability.** A test asserting `eggwork_core::NodeStatus` still deserializes a status response that carries only the existing fields, and that `active_executions` still means permits held. If the wire type is changed in any way, this test must fail.
- **Configuration fault taxonomy.** One test per rule, asserting the reported rule identifier and the operator-facing message.
- **Redaction under failure.** A test that constructs configurations with a private key path and a certificate fingerprint, forces a configuration fault, and asserts the resulting message contains neither the private-key path nor the fingerprint, nor any certificate subject. This test is the proof that the added detail did not become a leak, and it is required, not optional.
- **Consistency across surfaces.** A test asserting that `config print`, `config validate`, `doctor`'s configuration check, and the daemon's startup failure report the same rule identifier for the same fault.
- **Success is unaffected.** A test asserting that a valid configuration still loads and still prints with the existing redactions intact.

## 9. Required verification

The repository's pre-submit gate, run in full:

```
python3 scripts/check_execution_ownership.py
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets
cargo check --workspace
python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release
git diff --check
```

Plus the focused evidence:

- the metric-coverage test, run against the pre-change code, to show it **fails** before the fix and passes after;
- the naming-collision and distinct-name tests, run against the pre-change code, to show they fail before the fix;
- the redaction-under-failure test, run against the pre-change code and against any intermediate implementation, to show it never regresses.

The ownership guard is load-bearing here: `crates/eggwork-server/src/**` is scanned by `scripts/check_execution_ownership.py`, and this plan must not introduce a process-spawning dependency into the server crate to implement anything in §5.

`git diff --check` is part of the gate because this plan's own subject — citing code by symbol rather than by line — is a whitespace-and-formatting-adjacent discipline, and because the pre-submit list in `CONTRIBUTING.md` includes it.

## 10. Documentation updates

- `architecture/operations-cli.md` — update the inspection and reporting section to state that every incrementable metric is reported and that the vocabulary is owned by the producers; update the `doctor` description to reflect the current check list; update the output-discipline section to describe the configuration fault taxonomy and to state that a fault message names the rule and the item's role and discloses no path, fingerprint, or underlying error text.
- `architecture/core-domain.md` — state the meaning of `NodeStatus::active_executions` (permits held) and note that it is distinct from the operator-side non-terminal execution count.
- `architecture/server-node.md` — state that the permit count and the non-terminal row count are independent measurements that may legitimately disagree, and why.
- `docs/operations.md` — the operator-facing status and execution-listing output must document both distinctly named facts and say which is which; the `config print` / `config validate` section must show a failure message and explain the redaction rule.
- `docs/quickstart.md` — if any command in it can produce a configuration fault whose new text is shown, update the shown text.
- `architecture/overview.md` — the divergences table gains entries for the two findings that were recorded nowhere else (the metric vocabulary and the `active_executions` name collision) as *resolved by M008a*, or is corrected if it already carries them. This plan does not modify `architecture/overview.md`; the entry is recorded by the implementing agent under the roadmap's authority, or by the owner of that document.
- No document in this repository gains a line-number citation as a result of this plan. Citations added by this plan name the symbol and the file.

## 11. Acceptance criteria

1. `eggworkd metrics` and `eggworkd status` report `workspace_manifest_registrations`, `workspace_derived_requests`, `workspace_derived_hits`, `workspace_patch_entries`, and `workspace_base_missing`.
2. A test fails if any metric the server can increment is not reported by the operator surface. The test is demonstrated failing against the pre-change code.
3. The operator status output carries the permit count and the non-terminal execution count under two distinct names; neither uses `active_executions` for the non-terminal fact.
4. `eggwork_core::NodeStatus` is unchanged: same fields, same names, same meaning; `active_executions` remains permits held; a deployed client deserializing a status response is unaffected.
5. The operator output's disposition of the old `active_executions` key is decided, implemented, and documented, with the release-visible consequence stated.
6. A configuration fault produces a message naming the rule, the offending file or path (the role of the item, where the path is secret-bearing), and the corrective action.
7. No configuration fault message contains a private-key path, a certificate fingerprint, or a certificate subject. Proven by test.
8. `config print`, `config validate`, `doctor`, and daemon startup report the same rule identifier for the same fault.
9. `config print`'s existing redaction behaviour is unchanged.
10. The full pre-submit gate passes, with the focused tests demonstrated failing before the fix.
11. `architecture/operations-cli.md`, `architecture/core-domain.md`, `architecture/server-node.md`, `docs/operations.md`, and (if affected) `docs/quickstart.md` are updated and cite symbols, not line numbers.
12. No authentication, lease, generation, fencing, or digest bound is weakened; no new dependency is added; the scheduler-free fixed-target boundary is unchanged.

## 12. Stop conditions

Stop and escalate rather than proceeding if:

- the wire field `NodeStatus::active_executions` cannot be preserved without a wire-visible change that breaks deployed clients — in that case the implementer must not rename it unilaterally; escalate the compatibility question;
- the metric vocabulary cannot be given a single owner without a change to the node metrics table's on-disk format — that is a storage change and belongs to the storage journal's authority, not to this plan;
- making a configuration fault actionable would require disclosing a private-key path, a fingerprint, a subject, or arbitrary underlying error text — the redaction property wins; the operator-facing detail must be re-derived from the classified rule instead;
- a configuration failure would have to name a secret-bearing *value* to be actionable — out of scope; report the rule and the role only;
- the fix would change which configuration faults are fatal, or would change admission, authorization, lease, generation, or fencing behaviour;
- the work would require weakening the ownership guard, adding a process-spawning dependency to `eggwork-server`, or moving the fixed-target, scheduler-free boundary.

## 13. Closure evidence required

Create `plans/closure/operations-distribution/008a-status.md`.

Per `plans/003-planning-process.md` §6, the record must distinguish code inspection from executed evidence and must include:

- the reviewed repository head and the implementation commits;
- an executive finding;
- a requirement-to-evidence matrix mapping each acceptance criterion above to the specific command output or test that satisfies it;
- the exact focused and broader verification actually run, including the full pre-submit gate output;
- the platform matrix actually exercised, stated honestly — do not claim a platform was qualified merely because it compiles;
- negative and adversarial evidence: the metric-coverage test, the naming-collision test, and the redaction-under-failure test shown failing against the pre-corrective behaviour and passing against the post-corrective behaviour;
- the compatibility review: the disposition of the operator-side key rename, the confirmation that the wire type is byte-for-byte unchanged, the enumeration of in-tree consumers of `NodeStatus`, and the release-visibility consequence for operator scripts;
- the security review: confirmation that no configuration fault message disclosed a path, fingerprint, subject, or secret-bearing value;
- documentation status, listing each document updated;
- unresolved findings ranked by severity, including the blob-inspection shard-layout duplication recorded in §3;
- the registry and roadmap disposition.

Do not claim tests were run when they were only inspected. Do not claim a platform or feature is qualified merely because its code compiles.

## 14. Handoff notes

- **M008a is `proposed`, not `ready`.** It has no unmet dependency, but it carries two decisions that need a human's judgement before implementation: the exact names for the two operator-side facts (§5.2) and the disposition of the old `active_executions` key (§7). Resolve those first; the rest follows mechanically.
- **M008a is independent of M007 and of M008b.** It may execute in parallel with M007, with M008b, and with the other corrective addenda in this planning batch. It does not touch the service-host lifecycle, the shutdown-and-wait convergence contract, or the Windows service path; the foreground-`run` shutdown asymmetry is owned by M007 and is deliberately not duplicated here.
- The metric work in §5.1 is the part with the highest ratio of value to effort and should be done first, because it is the only finding in this plan that is purely additive and carries no compatibility cost.
- The redaction tension in §5.3 is the part most likely to be got wrong. The property in `architecture/operations-cli.md` — that no filesystem path, key detail, or child-process output reaches stderr — is deliberate and must survive. A configuration failure message that names a rule is worth having; a configuration failure message that leaks a key path is a security regression. The required test in §8 is what keeps the two apart.
- The repository's own experience is that a rule written in one module and re-implemented in another has drifted before, and the drift was found by inspection rather than by a failing test. Prefer the test that fails when the rule is weakened over a comment that describes the rule — that is the whole shape of OP-1 and OP-2.
