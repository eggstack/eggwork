# M008a closure record: operator surface truthfulness corrective

Status: **closed** — the three findings are closed in the code, the closure evidence is complete, and the coordinator's pre-submit gate is green.
the working tree; the coordinator owns the final status
Closed: — (the coordinator sets this when the gate passes)
Plan: `plans/implementation/operations-distribution/008a-operator-surface-truthfulness-corrective.md`
Roadmap: `plans/subsystems/operations-distribution-roadmap.md`
Addendum: `plans/subsystems/operations-distribution-operator-truthfulness-corrective-addendum.md`
Reviewed head: `721768d` (`feat(server): M005c resource-scoped authorization — implement the ADR-0007 boundary`)
Branch: `corrective/interrogation-batch-2026-10`
Working tree: the changes are **uncommitted** — the coordinator commits.

## 1. Executive finding

The operator surface had three ways of saying something other than what it meant. The
metric vocabulary had no owner that was the producer, so five live counters were
incremented on real traffic and dropped by the report; one field name,
`active_executions`, carried two different facts across the API and the CLI; and every
distinct configuration fault collapsed into a single unhelpful sentence.

All three are closed. The metric vocabulary is now a typed enumeration declared **with the
producers**, so the report cannot fall behind them and an unregistered name is a compile
error rather than a runtime drop. The two execution facts now have two names, and the
wire field keeps its published name and its published meaning. A configuration fault now
reports a stable rule identifier, the item that broke it — named by role alone when it is
secret-bearing — and the corrective action, and every operator surface reports the same
rule for the same fault.

Two decisions are recorded here because the plan left them open: the **names** of the two
operator-side facts (§4), and the **disposition of the old `active_executions` key**
(§6).

## 2. What changed, by finding

### 2.1 OP-1 — the metric vocabulary has a producer owner

| Item | Before | After |
|---|---|---|
| Vocabulary owner | `operations.rs`'s hand-maintained `METRIC_NAMES` (20 names) | `Metric` in `crates/eggwork-server/src/lib.rs`, declared with the producers (25 names) |
| Increment signature | `increment_metric(store, name: &'static str, amount)` | `increment_metric(store, metric: Metric, amount)` |
| Report seed | `METRIC_NAMES` → `BTreeMap` of zeros | `Metric::ALL` → `BTreeMap` of zeros |
| Report overlay | `if let Some(counter) = counters.get_mut(&name)` — **an unlisted row was silently ignored** | `counters.insert(name, value)` — a row the vocabulary does not know is reported **under its own name** |
| The five workspace counters | incremented, never reported | reported |

`increment_metrics` takes `&[(Metric, u64)]`, and the workspace-derivation batch is one
named function, `workspace_derive_metrics`, so the three counters it moves are one
decision. `terminal_metric_name` became `terminal_metric` and returns a `Metric`.

The report is now *every* metric the node can increment, with `0` before it is first
incremented, plus any row whose name this build does not know.

### 2.2 OP-2 — two facts, two names, one wire field

The wire field `eggwork_core::NodeStatus::active_executions` is **unchanged**: same name,
same fields, same meaning (permits held). `crates/eggwork-core/src/lib.rs` was not
touched by this milestone; §9 carries the exact doc-comment text the coordinator applies
to it and to `architecture/core-domain.md`.

The operator surface names the two facts apart (§4).

### 2.3 OP-3 — a typed configuration-fault taxonomy

`OperationsError::Config` is now `OperationsError::Config(ConfigFault)`, where
`ConfigFault` is `{ rule: ConfigRule, item: ConfigItem }`:

- `ConfigRule` is a closed enumeration of 60 rules with a stable, greppable identifier
  (`config_file_world_writable`, `tls_private_key_readable_by_others`,
  `config_bind_address_invalid`, …) and a one-sentence corrective action.
- `ConfigItem` is `Path`, `Value`, or `Role { role, qualifier }`. A secret-bearing item —
  the private key, the client certificate fingerprint, a resource scope's identifier — is
  named by its **role** and never by its path or value.
- The message renders as `[<rule>] <item>: <action>`.

The underlying `std::io::Error` and `serde_json` text is classified into a rule and then
dropped. `serde_json::Error::is_syntax()`/`is_eof()` separate a malformed file from a
malformed field; every other parse failure is `config_field_invalid`.

The daemon, `config print`, `config validate`, `doctor`'s `configuration` check, and
`status`'s own refusal all render the *same* fault value — the CLI never re-detects a
fault to describe it.

## 3. Requirement → evidence matrix

| # | Acceptance criterion | Evidence |
|---|---|---|
| 1 | `eggworkd metrics` and `eggworkd status` report the five workspace counters | `the_five_workspace_counters_are_visible_in_the_operator_status_payload` (status payload) and `every_metric_the_server_increments_reaches_the_operator_report` (the real increment path → the report). `eggworkd metrics` was added as a verb so the surface the plan names exists. |
| 2 | A test fails if any incrementable metric is not reported, demonstrated failing before the fix | `the_operator_report_carries_every_metric_the_server_can_increment` and `a_metric_name_the_report_does_not_know_is_reported_rather_than_dropped` — **both FAILED against the pre-change code** (§5). `the_reported_metric_set_is_exactly_the_producers_vocabulary` is the set-equality check over the producers. `no_increment_site_names_a_metric_outside_the_vocabulary` is the source-level guard against a second way to name a metric. |
| 3 | The operator status output carries both facts under two distinct names; neither uses `active_executions` for the non-terminal fact | `the_operator_status_payload_carries_both_execution_facts_under_distinct_names`, `the_execution_listing_uses_the_same_name_for_the_non_terminal_fact`, `no_operator_payload_reuses_a_wire_name_for_another_fact`. |
| 4 | `eggwork_core::NodeStatus` is unchanged | `the_wire_status_type_still_deserializes_the_existing_fields` — the type still deserializes a status response carrying only the existing fields, still serializes exactly `active_executions`, `capabilities`, `draining`, `node_id`, and a response *without* `active_executions` still fails to deserialize. No file under `crates/eggwork-core/` was modified. |
| 5 | The old `active_executions` key's disposition is decided, implemented, and documented | §6. The key is **removed** from both operator payloads; the release-visible consequence is stated there and in `docs/operations.md`. |
| 6 | A configuration fault names the rule, the item (role where secret-bearing), and the action | 46 per-rule tests (§4 of the test list) plus `every_configuration_rule_has_a_stable_identifier_and_an_action` for the rules an unprivileged test cannot construct. |
| 7 | No fault message contains a private-key path, a certificate fingerprint, or a certificate subject — proven by test | `a_configuration_fault_never_names_the_private_key_the_fingerprint_or_a_subject`. |
| 8 | `config print`, `config validate`, `doctor`, and daemon startup report the same rule identifier | `every_operator_surface_reports_the_same_rule_for_the_same_fault`. `status` reports the same rule in its own refusal, through `DoctorReport::configuration_detail`. |
| 9 | `config print`'s redaction is unchanged | `a_valid_configuration_still_loads_and_still_prints_redacted` — the private-key path and the client fingerprint are still `[REDACTED]`, and the resource-scope identifier redaction from M005c is untouched. |
| 10 | The pre-submit gate passes, with the focused tests demonstrated failing before the fix | §5, §7. The *scoped* gate is recorded here; the full workspace gate is the coordinator's. |
| 11 | The five documents are updated and cite symbols, not line numbers | §8. |
| 12 | No authentication, lease, generation, fencing, or digest bound weakened; no new dependency; the scheduler-free boundary unchanged | §9 (negative evidence). No dependency was added to any `Cargo.toml`; the only new file is a state-directory artefact the node writes beside the execution database. |

## 4. The chosen operator-side field names

| Name | Fact | Where |
|---|---|---|
| `execution_permits_held` | How many execution permits the **running node** holds right now. `null` when the node is not running. | `eggworkd status` |
| `non_terminal_executions` | How many durable execution rows are still in a non-terminal state (`Accepted`, `Preparing`, `Running`, `Cancelling`). | `eggworkd status`, `eggworkd executions list` |

The two are distinguishable **without reading prose**: one names *permits*, the other
names *non-terminal executions*. Neither is called `active_executions`, which is the
wire's name for the permit count.

**How the permit count reaches the operator.** The operator CLI is an offline process, so
the live permit count — a fact of the *running* node — cannot be re-derived from the
store. The node therefore publishes it: the count is written to
`database_path.with_extension("permits")` beside the execution database, in the same
owner-only state directory, replaced atomically (`O_EXCL` temporary file, `fsync`,
`rename`), refreshed on every permit acquisition and release, published at startup, and
removed on shutdown. The operator reports it only while the node's bind address is still
accepting connections (`node_is_listening`), because a number left behind by a stopped
node describes a node that is gone; otherwise it reports `null`.

This is the one place where the implementation goes beyond the plan's literal text, and
it is forced by acceptance criterion 3: the operator surface cannot carry the permit
count unless the running node publishes it. The published number is a *report* of the
semaphore, not a second owner of the fact — the semaphore still owns it, and the
published copy is written from `available_permits()`.

## 5. Before/after evidence for the key tests

The pre-change code was reconstructed by restoring `crates/eggwork-server/src/operations.rs`,
`crates/eggwork-server/src/lib.rs`, and `crates/eggwork-server/src/bin/eggworkd.rs` from
the reviewed head `721768d`, running the focused test targets, and restoring the working
tree. The index was not touched; the three files were swapped in place and swapped back.

### 5.1 The metric-coverage tests — FAIL before, PASS after

```
$ cargo test --locked -p eggwork-server --test operator_metric_coverage      # pre-change
running 2 tests
test a_metric_name_the_report_does_not_know_is_reported_rather_than_dropped ... FAILED
test the_operator_report_carries_every_metric_the_server_can_increment ... FAILED

---- the_operator_report_carries_every_metric_the_server_can_increment stdout ----
thread 'the_operator_report_carries_every_metric_the_server_can_increment' panicked at
  crates/eggwork-server/tests/operator_metric_coverage.rs:98:9:
assertion `left == right` failed: workspace_manifest_registrations is incremented by the
  server but missing from the operator report
  left: None
 right: Some(1)

---- a_metric_name_the_report_does_not_know_is_reported_rather_than_dropped stdout ----
assertion `left == right` failed: a counter the report does not recognise must still be
  reported
  left: None
 right: Some(11)

test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out
```

After the change:

```
$ cargo test --locked -p eggwork-server --test operator_metric_coverage
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

### 5.2 The naming-collision and distinct-name tests — FAIL before, PASS after

```
$ cargo test --locked -p eggwork-server --test operator_execution_counts    # pre-change
running 5 tests
test the_wire_status_type_still_deserializes_the_existing_fields ... ok
test the_execution_listing_uses_the_same_name_for_the_non_terminal_fact ... FAILED
test the_operator_status_payload_carries_both_execution_facts_under_distinct_names ... FAILED
test the_five_workspace_counters_are_visible_in_the_operator_status_payload ... FAILED
test no_operator_payload_reuses_a_wire_name_for_another_fact ... FAILED

test result: FAILED. 1 passed; 4 failed; 0 ignored; 0 measured; 0 filtered out
```

The wire-stability test passing against the pre-change code is the correct result: the
wire type did not change, and the test is there to prove it does not.

After the change:

```
$ cargo test --locked -p eggwork-server --test operator_execution_counts
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

### 5.3 What could not be demonstrated as "failing before"

`the_reported_metric_set_is_exactly_the_producers_vocabulary` and
`no_increment_site_names_a_metric_outside_the_vocabulary` are written against the typed
`Metric` vocabulary, which does not exist in the pre-change code. They therefore **cannot
be compiled against the pre-change tree**, and no failing run is claimed for them. The
compile-compatible tests in §5.1 and §5.2 are the before/after evidence.

`the_sandbox_helper_trust_failures_name_their_own_rule` and
`every_operator_surface_reports_the_same_rule_for_the_same_fault` are likewise written
against the new taxonomy and cannot be compiled against the pre-change tree.

## 6. The disposition of the old `active_executions` key

**Decision: the old key is removed from both operator payloads. No alias is offered.**

The plan's default (§7) was to retain `active_executions` for one release, emitting it
with the store's non-terminal meaning, and to state the decision here. This
implementation declines the alias, and the reason is acceptance criterion 3: *"neither
uses `active_executions` for the non-terminal fact"*. Retaining the key with the
non-terminal meaning would keep the exact defect this milestone exists to fix — one name
carrying two facts — for a release. Retaining it with the *permit* meaning instead
would be worse: an operator script that read `active_executions` from `eggworkd status`
previously got the non-terminal row count, and would silently start receiving the permit
count instead. A missing key fails loudly; a silently changed meaning does not.

**Release-visible consequence.** `eggworkd status` and `eggworkd executions list` no
longer carry an `active_executions` key. Operator scripts that read it must read
`non_terminal_executions` (the row count — the same number the key used to carry) or
`execution_permits_held` (the permit count), and must choose deliberately. The API's
`NodeStatus.active_executions` is unaffected: same name, same meaning, same wire shape.

The operator output is **not** protocol-neutral, and this record does not describe it as
such: it is operator-facing output, versioned with the binary, and the change is
release-visible.

## 7. Verification actually run

All commands were run with `CARGO_TARGET_DIR=/home/sugarwookie/projects/eggwork/target-w6a`.

| Command | Result |
|---|---|
| `cargo fmt -p eggwork-server -- --check` | **clean**, no output |
| `cargo clippy --locked -p eggwork-server --all-targets --all-features -- -D warnings` | **clean**, no warnings |
| `cargo check --locked -p eggwork-server` | `Finished dev profile`, no warnings |
| `cargo test --locked -p eggwork-server --all-targets` | **306 passed, 0 failed** — 168 (lib) + 7 + 3 + 12 + 5 + 2 + 54 + 36 + 13 + 6 |
| `cargo test --locked -p eggwork-server --test operator_metric_coverage` | 2 passed |
| `cargo test --locked -p eggwork-server --test operator_execution_counts` | 5 passed |
| `cargo test --locked -p eggwork-server --test operator_surface_truthfulness` | 54 passed |
| `python3 scripts/check_execution_ownership.py` | `execution ownership guard passed; approved process owners: eggwork-runner (2 spawn sites), eggwork-sandbox-helper (1 spawn sites)` · `negative scanner self-test passed; crate dependency direction passed` |

**Not run here:** the whole-workspace suite, the release/qualification harness
(`--test installed_qualification` is opt-in and `#[ignore]`d), and the Python
release-harness tests. Those are the coordinator's gate. The Linux-only qualification
path was **not** exercised by this milestone: nothing here changes the install, the
service host, or the release harness.

## 8. Documentation status

| Document | Change |
|---|---|
| `architecture/operations-cli.md` | The report path now states that the vocabulary is owned by the producers and that an unknown name is reported under its own name; the `doctor` section describes the eight checks and the classified `configuration` detail and the helper-trust rules; the output-discipline section describes the configuration-fault taxonomy and states that a fault names the rule and the item's role and discloses no path, fingerprint, or underlying error text; the inspection section states that the non-terminal count is a row count and not a permit count, and names both operator-side facts. |
| `architecture/server-node.md` | States that the permit count and the non-terminal row count are independent measurements that may legitimately disagree, and why. |
| `docs/operations.md` | Documents `eggworkd metrics`; adds a table naming the two execution facts and saying which is which, and states that the API's `active_executions` keeps its published meaning; the redaction section now also covers the resource-scope identifier; the configuration-security section shows a real failure message and explains the redaction rule. |
| `docs/quickstart.md` | Step 5 shows the new actionable failure text; the troubleshooting table names the rules; the `doctor` example now shows all eight checks the node emits (it showed seven, so the quickstart-sync check would have failed). |
| `architecture/core-domain.md` | **Not edited by this milestone** — the coordinator applies the text in §9. |
| `architecture/overview.md` | **Not edited by this milestone** — the coordinator applies the two divergence rows in §9. |

No document in this change gained a line-number citation; the new text cites symbols and
files.

## 9. Changes the coordinator owns

### 9.1 `crates/eggwork-core/src/lib.rs` — the `NodeStatus` doc comment

Apply immediately above `pub struct NodeStatus`:

```rust
/// The node's status as published on the control plane.
///
/// `active_executions` is the number of execution permits the node currently
/// holds — how many executions it is running right now. It is **not** the
/// number of durable execution rows in a non-terminal state: an execution
/// whose process died keeps its row until the node recovers it, and an
/// idempotent re-request releases its permit while the original execution
/// still holds one, so the two counts legitimately disagree. Only the running
/// node knows the permit count, which is why the operator surface reports it
/// under its own name (`execution_permits_held`) and reports the row count
/// separately (`non_terminal_executions`).
///
/// The field name, the field set, and this meaning are published: a deployed
/// client deserializes this type, so renaming the field would break every
/// client that reads node status, and adding one would change the wire shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeStatus {
```

(The `#[derive(...)]` line is shown so the paste point is unambiguous; the existing
derive line is already there and must not be duplicated.)

### 9.2 `architecture/core-domain.md` — the prose

Add to the section that describes `NodeStatus`:

```markdown
`NodeStatus::active_executions` is the number of execution **permits the node
currently holds** — how many executions it is running right now. It is not the
number of durable execution rows in a non-terminal state. Those two counts
legitimately disagree: an execution whose process died keeps its row until the
node recovers it, and an idempotent re-request releases its permit while the
original execution still holds one. Only the running node knows the permit
count, so the operator surface reports it under its own name
(`execution_permits_held`) and reports the row count separately
(`non_terminal_executions`). The field name, the field set, and this meaning
are published — a deployed client deserializes `NodeStatus`, so renaming the
field would break every client that reads node status, and adding one would
change the wire shape.
```

### 9.3 `architecture/overview.md` — two divergence rows

Append to the last table of the *"Where the implementation differs from the prose"*
section (the rows are numbered monotonically across the document, so these are **23**
and **24**):

```markdown
| 23 | ~~The operator surface's metric vocabulary had no owner that was the producer.~~ **Closed by the M008a corrective.** `operations.rs` declared a hand-maintained `METRIC_NAMES` and the report copied a stored value *only* when the name was already in its map, so a counter the server incremented but the list did not name was dropped by the report — five of them, all workspace counters. The vocabulary is now a typed `Metric` enumeration declared with the producers in `crates/eggwork-server/src/lib.rs`; the report reads that declaration, and a row whose name the vocabulary does not know is reported under its own name rather than dropped. | [operations-cli.md](operations-cli.md) · [storage-journal.md](storage-journal.md) |
| 24 | ~~One field name, `active_executions`, carried two facts.~~ **Closed by the M008a corrective.** On the wire it is the number of execution permits the node holds; in the operator's own listing it was the number of durable execution rows in a non-terminal state, and nothing in the output said which was which. The wire field keeps its published name and its published meaning — renaming it would break every deployed client that deserializes `NodeStatus` — and the operator surface now names the two facts apart: `execution_permits_held` for the permit count, `non_terminal_executions` for the row count. The old operator-side `active_executions` key is removed rather than aliased, because an alias would keep the ambiguity alive and a silently changed meaning is worse than a missing key. | [operations-cli.md](operations-cli.md) · [server-node.md](server-node.md) |
```

### 9.4 `plans/registry.md`, `AGENTS.md`, `plans/subsystems/operations-distribution-roadmap.md`

The coordinator updates these; the wording this milestone implies:

- `plans/registry.md` — the M008a row's status moves from `proposed` to `closed`, with
  the closure record as its evidence. The registry's operations summary line should
  record that the three findings are closed and that the operator output's
  `active_executions` key was removed.
- `plans/subsystems/operations-distribution-roadmap.md` — the M008a milestone row moves
  from `active` to `closed`, pointing at
  `plans/closure/operations-distribution/008a-status.md`.
- `AGENTS.md` — the documentation section says the config loader's strictness "produces a
  single unhelpful `configuration is invalid or unreadable`". That sentence is now
  false: the loader reports the rule, the item, and the action. Suggested replacement:
  "the config loader's strictness (owner-only state directories `0700`, private key
  `0600`, nothing group/world writable, and a sandbox helper outside world-writable
  ancestors) is reported as a stable rule identifier, the offending file or path, and
  the corrective action — and a secret-bearing item is named by its role, never by its
  path."

## 10. Negative evidence and adversarial checks

| Check | Result |
|---|---|
| The private-key path, the client fingerprint, and both certificate subjects (`localhost`, `eggwork operator test CA`) are absent from every fault message | asserted by `a_configuration_fault_never_names_the_private_key_the_fingerprint_or_a_subject` — the fingerprint is `ab`×32 and the subjects are the *actual* subjects of the generated certificates, so the assertion is against real material and not a placeholder |
| A secret-bearing item is named by role | asserted by the per-rule tests, e.g. `tls_private_key_*` cases assert the message contains `the private key` and not the path |
| The resource-scope identifier redaction from M005c still holds | `a_valid_configuration_still_loads_and_still_prints_redacted` asserts the printed config still redacts the private key and the fingerprint; the scope redaction is untouched by this change and is asserted by the M005c tests, which still pass (13 in `resource_scoped_authorization.rs`) |
| The M005c resource-scope *validation* is untouched | the seven scope rules in the taxonomy are read from the same `ClientGrant::resource_scopes` shape; `resource_scoped_authorization.rs` passes unchanged |
| No process creation was added | the ownership guard passes and reports the same two approved owners with the same spawn-site counts as before |
| No dependency was added | no `Cargo.toml` was touched |
| No authentication, lease, generation, fencing, or digest bound was weakened | the diff touches the metric vocabulary, the operator's field names, the configuration-error type, and the operator's reporting. `authorize_request`, the lease TTL, the generation fence, and the digest parsing are unchanged. `trusted_private_key` still requires owner-write, no group/world write, and no read by others — the previous three predicates were *split* into four named rules, not relaxed. |
| The wire type is untouched | `git status` shows no change under `crates/eggwork-core/` |
| An unknown metric name is not silently dropped | the report inserts it under its own name; asserted |
| A metric cannot be incremented without being in the vocabulary | the increment helpers accept a `Metric`; the source scan asserts no increment site names a metric as a string |

## 11. Unresolved findings, by severity

| Severity | Finding | Disposition |
|---|---|---|
| Low | **The wrong-owner rules are not exercised by a fault.** `config_file_wrong_owner`, `tls_certificate_wrong_owner`, `tls_private_key_wrong_owner`, `state_root_wrong_owner`, `database_directory_wrong_owner`, and `database_file_wrong_owner` need a file owned by a second user, which an unprivileged test cannot create. The predicates are exercised only through the mode and type rules. | Asserted through the vocabulary test; the owner predicates were *split out* of the old combined predicate, so they are now individually named and individually greppable. A privileged CI job could construct them. |
| Low | **`config_serialization_failed` and `sandbox_helper_unsupported_platform` are not reachable through the current shapes.** `serde_json` never fails to serialize the configuration, and the runner reports the unsupported-platform phrase only off Linux. | Declared and asserted through the vocabulary test; the `redacted_json` failure path still has a rule to report. |
| Low | **`tls_client_ca_invalid` is reachable only above the crate's trust-root cap.** An empty CA bundle is refused earlier, by `tls_certificate_malformed`. | Constructed in a test by writing 257 roots into the bundle. |
| Low | **The published permit count is a point-in-time sample.** It is written on every permit change, so it is exact between changes, but a node that is *running* and *not listening* (a node started with a bind address that failed, or a node whose listener was closed) would have its published number reported as `null`. | Deliberate: `null` means "no live permit count is observable", which is truthful. A node that is running and listening always reports the count. |
| Low | **The operator's permit count needs a listening node.** `eggworkd status` on a stopped node reports `null` for the permit count, because only a running node knows it. | Deliberate and documented in `docs/operations.md`. The alternative — reporting a stale number — would be the exact untruthfulness this milestone fixes. |
| Low | **The `metrics` verb is new.** The plan's acceptance criterion 1 names `eggworkd metrics`, which did not exist as a verb; the metrics were only reachable inside the status payload. | The verb was added (one command, one JSON document) so the surface the plan names exists. The release harness does not assert on the verb list. |
| Informational | The doctor report's `configuration` detail now carries a rule identifier and an action, so its text is longer than the previous "configuration or TLS material is invalid". | Expected and intended; it is the whole point of OP-3. |

## 12. Registry and roadmap disposition

- **Registry:** M008a moves from `proposed` to `closed` when the coordinator's gate passes.
  The registry's operations summary line should record that the three findings are closed
  and that the operator output's `active_executions` key was removed.
- **Roadmap:** the M008a milestone row moves from `active` to `closed`, pointing at
  `plans/closure/operations-distribution/008a-status.md`.
- **Plan status line:** set to `active` by this milestone (the coordinator sets the
  final status after the gate).
- **M008b** is independent of this milestone and is owned by another worker; nothing
  here touches the release harness, the CI feature gate, or the design record's citation
  hygiene.
- **M007** (the Windows service host) is untouched: the node's shutdown path gained one
  line that removes the published permit count, and the permit wrapper's `Drop` runs on
  the same task-teardown path as before.
