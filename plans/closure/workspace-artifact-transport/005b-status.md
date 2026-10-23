# Workspace and Artifact Transport M005b Closure — GC Preview and Store Ownership Corrective

Source plan: `plans/implementation/workspace-artifact-transport/005b-gc-preview-and-store-ownership-corrective.md`
Corrective addendum: `plans/subsystems/workspace-artifact-transport-data-plane-corrective-addendum.md`
Subsystem roadmap: `plans/subsystems/workspace-artifact-transport-roadmap.md`
Reviewed head at the start of the milestone: `6f41957` (`feat(security): M005a degraded isolation observability`)
Head when the milestone work was finished: `a8abfcb` (`docs(plans): record the core protocol-contract ownership change as ADR-0008`) — a coordinator-owned documentation commit that does not touch any file in this milestone's change set.
Implementation commit: `63699df`
Closure record commit: `63699df`

## Finding

The operator's GC dry-run preview and the apply pass used **different candidate predicates**, in a module whose own doc comment states that the two must be shared. The owner module (`blob.rs`) already said so: *"Shared by the dry run and the real run on purpose... Two hand-copied copies of it are what let the two paths drift apart."* The rule was violated in the sibling module: `operations.rs` carried a hand-copied preview query that selected on `blobs.reference_count = 0` while the collector selected on the absence of a reference row. The counter is a denormalization maintained only by the `blob_reference_insert` / `blob_reference_delete` triggers, so it can drift from the rows that actually pin bytes; the preview could therefore report a candidate the apply pass would not remove, and miss one it would.

Three further ownership defects sat alongside it. The blob store's path layout was re-derived in the operator (`config.blob_root.join(&digest.as_str()[..2]).join(digest.as_str())`) instead of being taken from the store. The three store metadata file names were spelled in the operator and in `lib.rs`. And the `"{principal}:{manifest_digest}"` composite was written in three places, with one call site reached precisely when the stored principal does not parse as a `PrincipalId`.

A fourth defect was found while reading the cited sites: `materialize_tree` accepted blob bytes on a **size-only** check. A blob whose length matched the manifest but whose content did not match its digest was copied into the workspace tree.

The milestone closes all four. The candidate predicate now lives in the owner modules and is the only place it is written; the operator's preview obtains its candidate sets through that owner code and contains no SQL of its own; the preview reaches the owner through a non-mutating seam, because every owner `open` creates and reconciles; the blob layout and the metadata file names have one owner each; the manifest owner id has one producer; and the materializer verifies the bytes it writes while it copies them.

**Scope note.** This record is written from the implementing agent's working tree, with the milestone's code **not yet committed**. The implementation commit is a placeholder for the coordinator to fill in.

## What was inspected by reading, and what was executed

Everything in the "Implementation site" column below was established by reading the cited source at the reviewed head. Everything in the "Test" column was **executed** on the host described in [Platform and feature matrix actually exercised](#platform-and-feature-matrix-actually-exercised), with the exact commands and outcomes in [Verification actually run](#verification-actually-run).

Two claims in the plan's section 13 could not be satisfied as written and are recorded honestly in [Unresolved findings](#unresolved-findings-by-severity):

1. The corrupt-principal owner-id case **does not fail against the pre-fix code** — it was verified experimentally. The pre-fix inline format produced the identical composite, so the pre-fix code released the reference correctly. The defect was the *duplication* of the format, not a wrong release.
2. The workspace-wide gate required by acceptance criterion 13 (`cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` and `cargo test --locked --workspace --all-targets`) was **not run by this milestone**. The coordinator reserved the full gate for the end of the batch, and a second agent is concurrently editing the runner and the sandbox helper in the same tree. The scoped equivalents are recorded instead.

## Requirement-to-evidence matrix

One row per acceptance criterion in the plan's section 11.

| # | Acceptance criterion | Implementation site | Test |
|---|---|---|---|
| 1 | The dry-run report and the apply pass select candidates through the same store-owned statement; the operator module contains no candidate predicate of its own. | `blob.rs`: `UNREFERENCED_CANDIDATES_SQL`, `BlobStore::unreferenced_candidates`, `blob::preview_gc`. `workspace.rs`: `WORKSPACE_CANDIDATES_SQL`, `MANIFEST_CANDIDATES_SQL`, `workspace_candidates`, `manifest_candidates`, `workspace::preview_gc`. `artifact.rs`: `ARTIFACT_CANDIDATES_SQL`, `artifact_candidates`, `artifact::preview_gc`. `operations.rs`: `dry_run_gc` now calls only those owner entry points and holds no SQL. | `operations.rs`: `gc_preview_uses_the_stores_own_blob_predicate_not_the_reference_count_counter`. `tests/gc_preview_parity.rs`: `the_operator_module_contains_no_candidate_predicate_of_its_own`, `the_operator_preview_spells_no_sql_and_opens_no_store`. |
| 2 | The blob candidate predicate is the reference-row predicate, not the `reference_count` counter. | `blob.rs`: `UNREFERENCED_CANDIDATES_SQL` is `WHERE NOT EXISTS (SELECT 1 FROM blob_references r WHERE r.digest = blobs.digest)`. The counter appears nowhere in any reclamation query. | `operations.rs`: `gc_preview_uses_the_stores_own_blob_predicate_not_the_reference_count_counter` (a blob with a live reference row and a counter of zero is not a candidate; a blob with no reference rows and a counter of one is). Non-vacuity shown below. |
| 3 | The rule recorded on the doc comment of `BlobStore::unreferenced_candidates` is enforced by a test rather than stated only in a comment. | The doc comment was extended to name the third reader (the operator's preview) and to point at `preview_gc`. The function now takes a `&Connection` so the preview can evaluate it without a store instance. | `operations.rs`: `gc_preview_candidates_are_a_subset_of_what_the_apply_pass_removes` and `gc_preview_and_apply_agree_when_the_store_has_no_expired_references` assert the relation the comment states. |
| 4 | The preview remains non-mutating, proven on a store that does not yet exist. | `blob::preview_gc`, `workspace::preview_gc`, `artifact::preview_gc` open the metadata database with `SQLITE_OPEN_READ_ONLY`; they never call `BlobStore::open`, `WorkspaceManager::open` or `ArtifactStore::open`, each of which creates and reconciles. | `operations.rs`: `gc_preview_does_not_create_a_store_that_does_not_exist`. |
| 5 | A test proves the preview's candidate set is a subset of the apply pass's removed set, and is equal to it for the same inputs when the store contains no expired references. | The preview evaluates the candidate statement before the expired references are deleted; the apply pass evaluates it after. That difference is the reason the relation is a subset and not bare equality. | `operations.rs`: `gc_preview_candidates_are_a_subset_of_what_the_apply_pass_removes` (subset) and `gc_preview_and_apply_agree_when_the_store_has_no_expired_references` (equality). |
| 6 | `inspect_blob` resolves object paths through the store's path ownership, and its symlink and size-cross-check properties are preserved. | `blob.rs`: `blob::shard_path`, `blob::object_path` (pure, `(root, digest) -> PathBuf`); `BlobStore::path_for` delegates to `object_path`. `operations.rs`: `inspect_blob` uses `blob::shard_path` and `blob::object_path` and spells no layout of its own. | `operations.rs`: `inspect_blob_resolves_the_object_through_the_store_path_ownership` (resolves in a real store; a recorded size that disagrees with the file's length is `OperationsError::Data`), `inspect_blob_refuses_a_symlinked_shard_or_object` (unix: symlinked shard directory and symlinked object file are both refused). `tests/gc_preview_parity.rs`: `the_operator_module_contains_no_candidate_predicate_of_its_own` forbids `as_str()[..2]` in the operator. |
| 7 | The workspace materializer refuses blob content that does not match the manifest digest, even when the length matches, and the refusal is proven non-vacuous. | `blob.rs`: `BlobStore::copy_verified` hashes the byte stream it forwards and returns `BlobError::DigestMismatch`; `VerifyWriter` in `blob.rs` is the hashing writer. `workspace.rs`: `materialize_tree` uses `copy_verified`, maps a digest mismatch to `WorkspaceError::InvalidBlob`, and removes the staged file so unverified bytes do not survive. | `workspace.rs`: `materialize_tree_refuses_blob_bytes_that_do_not_match_the_manifest_digest` — the stored object is corrupted **in place, keeping its length**, and the materializer refuses it; the same test asserts correct bytes still materialize. Non-vacuity shown below. |
| 8 | The manifest owner-id composite has exactly one producer; both inlined call sites use it; the `&PrincipalId` versus `&str` mismatch is resolved by an explicit decision; and a corrupt-row case still releases its reference. | `workspace.rs`: `manifest_owner_id_for_key(principal_key: &str, manifest_digest: &str)` is the single implementation of the format; `manifest_owner_id(principal_id: &PrincipalId, manifest_digest: &str)` is the validated-principal entry point and delegates to it. Both former inline call sites (`manifest_garbage_collect` and the `reconcile_manifests` orphan sweep) route through them. The `&PrincipalId` / `&str` mismatch is resolved by validating the row's principal where validation is possible and falling back to the raw key otherwise — both produce the same bytes, so the released owner id is unchanged either way. | `workspace.rs`: `manifest_owner_id_has_one_producer_for_validated_and_string_principals` (the two entry points agree, including for a principal that itself contains a colon); `manifest_gc_releases_the_reference_for_an_unparseable_stored_principal` (a stored principal that does not parse still has its blob pin released). `tests/gc_preview_parity.rs` enforces that the operator holds no copy of a store's knowledge. See [Unresolved findings](#unresolved-findings-by-severity) for the honest non-vacuity status of the corrupt-principal case. |
| 9 | No public protocol surface changed: no route, request, response, capability string, `NodeGcReport` field, or `ResourceInspection` field. | `NodeGcReport` (`lib.rs`) keeps the same thirteen fields, names, types and order. `ResourceInspection` (`operations.rs`) keeps the same three fields, and the `metadata` JSON each `inspect` verb emits is unchanged. No route, request struct, response struct or capability string was touched. | Verified by inspection and by the pre-existing suites: `cargo test -p eggwork-server --lib` → 142 passed, 0 failed. `crates/eggwork-server/tests/release_contract.rs` and `installed_qualification.rs` were **not** executed by this milestone (see [Verification actually run](#verification-actually-run)). |
| 10 | No stored format changed: no schema, row, file name, shard layout, or composite format. | No `CREATE TABLE` statement changed in `blob.rs`, `workspace.rs` or `artifact.rs`. The blob shard layout is still `<root>/<first two hex>/<64 hex>`; `blob::object_path` computes the same path the old inline expression computed. The metadata file names are unchanged. The composite format is still `"{principal}:{manifest_digest}"`. The manifest digest algorithm and the artifact record format are untouched. | `operations.rs`: `inspect_blob_resolves_the_object_through_the_store_path_ownership` resolves an object at the same path the store writes it to. `workspace.rs`: `manifest_owner_id_has_one_producer_for_validated_and_string_principals` pins the composite's exact bytes. |
| 11 | The GC ordering, the bounded-per-pass semantics, the `clamp(1, 1024)` limit, and the apply pass's exclusive lock are unchanged. | `operations.rs`: `collect_garbage` still runs workspaces → manifests → artifacts → blobs, still clamps to `1..=MAX_GC_BATCH` (1024), and still takes the exclusive maintenance lock before applying. `lib.rs`: `NodeServer::collect_garbage` is unchanged apart from the artifact database path now coming from `artifact::metadata_path`. | `operations.rs`: `applying_gc_refuses_to_race_a_running_node` (the exclusive lock still refuses to race a running node) — passed as part of the 142-test lib run. `garbage_collection_preview_reads_only_and_reports_bounds` shows the preview's bound is honoured (one candidate at `limit = 1`, the full census at `limit = 8`). |
| 12 | No bound is widened; no authentication, lease, generation, fencing, or digest-bound semantics weakened; no queue, worker selection, priority, fairness, DAG, or semantic retry introduced. | No quota, size bound, entry-count bound, or retention horizon was changed. No authentication, authorization, lease, generation or fencing code was touched. No scheduler-like concept was introduced. The only new bounds are the *existing* `limit` applied to the preview's owner queries, which the preview already applied. | `scripts/check_execution_ownership.py` passed (no process creation added to `eggwork-server`). The pre-existing authorization, lease and fencing tests are part of the 142-test lib run and passed unchanged. |
| 13 | `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` and `cargo test --locked --workspace --all-targets` are green on the same head. | **Not satisfied by this record.** The workspace-wide gate was not run here; the coordinator reserved it for the end of the batch, and a second agent is concurrently editing `crates/eggwork-runner/**` and `crates/eggwork-sandbox-helper/**` in the same tree. The scoped equivalents are recorded in [Verification actually run](#verification-actually-run). | Pending the coordinator's full gate. |

## Decisions taken where the plan left the choice open

### The non-mutating preview seam

The plan offered three options: a read-only non-reconciling open; public side-effect-free candidate-query entry points on the owner modules; or a store-owned pure function.

**Chosen: public, side-effect-free candidate-query entry points on the owner modules** — `blob::preview_gc`, `workspace::preview_gc` and `artifact::preview_gc` — each of which opens the store's metadata database `SQLITE_OPEN_READ_ONLY` and evaluates the owner's own canonical statement.

Why this and not the alternatives:

- A *read-only non-reconciling open* would still have to skip `remove_stale_parts` and `reconcile`, which means a second `open` variant that differs from the one the collector uses. That is the same duplication the milestone is removing, one level up: the collector and the preview would then use different `open`s, and a future change to the reconciliation would have to be made in both.
- A *store-owned pure function* over the store's in-memory state requires a store instance, and every way of obtaining one — `BlobStore::open`, `WorkspaceManager::open`, `ArtifactStore::open` — creates and reconciles. The pure-function option is therefore only reachable through a non-mutating open, which is the first option.
- The chosen shape keeps the *statement* in the owner module, which is the property that actually matters: the preview and the collector evaluate the same SQL text, so they cannot answer different questions even if the surrounding plumbing changes.

The seam's cost is stated plainly: the preview reads the metadata database **directly** rather than through a store instance, so it sees the store's rows and not the store's in-memory reconciliation. For a preview that is the correct trade — the reconciliation is precisely what the preview must not perform.

The seam also preserves two behaviours that were already true and are now enforced rather than incidental: a preview against a store that does not exist **fails with a typed error** (`OperationsError::Data`, surfaced as the generic "bounded garbage collection failed") rather than degrading to zero candidates; and a store that predates the retained-manifest cache reports zero manifest candidates instead of failing the whole preview.

### Verify-while-copying, not verify-then-copy

**Chosen: verify-while-copying.** `BlobStore::copy_verified` hashes the byte stream it forwards into the staging file and compares the result with the manifest's digest. A verify-then-copy implementation would have hashed the object file, closed it, reopened it and copied it — a third read, and a window in which the object can change between the hash and the copy.

The performance consequence, stated precisely, because `architecture/data-plane.md` records that *"blob download and workspace materialization both pay a full re-hash of every input"*:

| | Reads of the object file | SHA-256 passes over the input |
|---|---|---|
| Before the change | 2 — one for the pre-flight `verify_existing`, one for the copy | 1 — the pre-flight re-hash |
| After the change (verify-while-copying) | 2 — unchanged | 2 — the pre-flight re-hash, plus one over the copied bytes |
| Verify-then-copy would have been | 3 — a third read of every object file | 3 |

So the change adds **one SHA-256 computation over the copied bytes per materialized file** and **no additional read** of the object and no additional bytes moved. The recorded property is unchanged: materialization still pays a full re-hash of every input, and that re-hash is now bound to the bytes that actually reach the tree rather than to a separate earlier read. The cost is CPU, not I/O.

### The manifest owner-id producer shape

**Chosen:** one implementation of the format, `manifest_owner_id_for_key(principal_key: &str, manifest_digest: &str)`, with two entry points — `manifest_owner_id(principal_id: &PrincipalId, manifest_digest: &str)` (validated principal, delegating to the raw form) and the raw form itself.

The `&PrincipalId` / `&str` mismatch is resolved explicitly rather than assumed away. Two call sites read the principal out of a SQL row, where it may not parse as a `PrincipalId` — and one of them, the `reconcile_manifests` orphan sweep, is reached *precisely* when the principal does not parse. Refusing to release the reference in that case would leak the pin: the blob would stay unreclaimable for ever. So the raw form is the fallback, and the validated form delegates to it. Because `PrincipalId::as_str()` returns the principal unchanged, both entry points produce the same bytes for the same principal, and the choice of entry point never changes which owner id is released.

The composite is `"{principal}:{manifest_digest}"`. A manifest digest is hex and contains no colon, so the pair is recovered by splitting at the **last** colon even when the principal itself contains one — `PrincipalId` permits `:` in a principal.

## The non-mutating-preview evidence

`gc_preview_does_not_create_a_store_that_does_not_exist` (`operations.rs`) points the preview at a store that does not exist:

1. It asserts that the blob root, the workspace root, the execution database, and **the database's parent directory** are all absent before the call.
2. It asserts `collect_garbage(config, true, 1)` is an error — the preview cannot evaluate a store that is not there, and it does not degrade to zero candidates.
3. It asserts that after the failed preview, all four paths are **still** absent: the preview created nothing.
4. It then performs `BlobStore::open` — the creating, reconciling open the preview must not use — and asserts that the blob root and its metadata database now exist. This step exists so the assertion in step 3 has teeth: it demonstrates that the creating open really would have brought the store into being.

The same property is asserted against a store that *does* exist: `garbage_collection_preview_reads_only_and_reports_bounds` writes an in-flight `.part-` upload into the blob root, runs the preview, and asserts the file is still there with its bytes intact.

## The preview/apply agreement evidence

The fixture builds the three stores through their owner modules (`BlobStore::open`, `WorkspaceManager::open`, `ArtifactStore::open`), so it is a real store. This replaces the previous fixture, which hand-built a `blobs` table whose columns were `(reference_count, size_bytes, last_used_unix_ms)` — with no `digest` column at all — and was therefore a database shaped to the hand-copied predicate. The canonical statement could not even be evaluated against it.

### The seeded blob states

| State | Reference rows | `reference_count` | Under the reference-row predicate | Under the old counter predicate |
|---|---|---|---|---|
| `live` | one live row | 1 | not a candidate | not a candidate |
| `expired` | one expired row | 1 | not a candidate *at preview time*; a candidate after the expired row is swept | not a candidate |
| `unreferenced-a/b/c` | none | 0 | candidate | candidate |
| `counter-low-a/b` | one live row | **0** (drifted) | not a candidate | **candidate** |
| `counter-high` | none | **1** (drifted) | **candidate** | not a candidate |

The drift is written directly into `blobs.reference_count`. That is not a fabricated reference row: it reproduces the real condition under which the trigger-maintained counter disagrees with the rows that actually pin bytes, which is the whole point of the divergence.

The fixture is **asymmetric on purpose** — two `counter-low` blobs against one `counter-high`. With a symmetric fixture the two predicates produce the same *count* and differ only in membership, and a count comparison cannot see the difference. With the asymmetry the reference-row predicate yields **4** candidates and the counter predicate yields **5**, so the operator's reported count alone discriminates.

### The candidate sets and the relation observed

`gc_preview_candidates_are_a_subset_of_what_the_apply_pass_removes` (store contains one expired reference):

- the owner's canonical statement selects **4** candidates: `unreferenced-a`, `unreferenced-b`, `unreferenced-c`, `counter-high`;
- the apply pass removes **5** objects: those four, plus `expired`;
- every one of the preview's four is in the apply pass's five — **the preview's set is a subset of what the apply pass removes**;
- the difference is exactly `expired`, and the reason is recorded in the owner module's doc comment: the preview selects *before* the expired reference rows are deleted, the apply pass selects *after*, so a blob that has just become unreferenced is collectable in the same pass;
- `live` and `counter-low-a/b` survive the apply pass — they are pinned, exactly as the reference-row predicate says.

`gc_preview_and_apply_agree_when_the_store_has_no_expired_references` (no expired reference in the store): the preview's set and the apply pass's removed set are **equal** — 4 candidates, 4 removed. This is the equality case the plan asks for, demonstrated separately.

`gc_preview_uses_the_stores_own_blob_predicate_not_the_reference_count_counter` closes the loop between the operator and the owner: the operator's reported `blob_candidates` (4) and `blob_candidate_bytes` equal the owner's canonical set exactly, and the membership assertions hold — `counter-low-a/b` are **not** candidates despite their counter saying zero, and `counter-high` **is** a candidate despite its counter saying one.

## Non-vacuity evidence

Each new test was made to fail against the pre-fix behaviour by deliberately reintroducing the defect, running the test, and then reverting. Every reversion was verified (the touched file's checksum was compared before and after).

| Test | Defect deliberately reintroduced | Observed outcome | Reverted |
|---|---|---|---|
| `gc_preview_uses_the_stores_own_blob_predicate_not_the_reference_count_counter` | The owner's statement reverted to `WHERE reference_count = 0` | **FAILED** — "a blob with a live reference row is never a candidate". The subset test failed in the same run ("assertion failed: !removed.contains(&states.counter_low...)"). Run result: 2 passed, 2 failed. | yes |
| `gc_preview_uses_the_stores_own_blob_predicate_not_the_reference_count_counter`, `gc_preview_candidates_are_a_subset_of_what_the_apply_pass_removes`, `gc_preview_and_apply_agree_when_the_store_has_no_expired_references` | The operator's preview given **its own** divergent statement (`WHERE reference_count = 0`) — the original defect's exact shape | **3 FAILED**, 1 passed. The two set tests failed with `left: 5, right: 4` (the operator's count under the divergent predicate against the owner's count). Only `gc_preview_does_not_create_a_store_that_does_not_exist` passed, as expected — that test is about mutation, not selection. | yes |
| `materialize_tree_refuses_blob_bytes_that_do_not_match_the_manifest_digest` | `copy_verified`'s digest check removed, leaving a size-only copy | **FAILED** — the assertion `matches!(materialize_tree(...), Err(WorkspaceError::InvalidBlob))` did not hold: the mismatched bytes were copied into the tree. | yes |
| `the_operator_module_contains_no_candidate_predicate_of_its_own` | *(by inspection of the pre-change file)* `git show HEAD:crates/eggwork-server/src/operations.rs` scanned for the eleven forbidden fragments | **10 of the 11 fragments were present** in the operator's production code: `reference_count = 0`, `expires_unix_ms IS NOT NULL`, `expires_unix_ms <= `, `ORDER BY expires_unix_ms`, `ORDER BY last_used_unix_ms`, `FROM retained_manifests`, `FROM blob_references`, `metadata.sqlite`, `artifacts.sqlite`, `as_str()[..2]`. The eleventh, `NOT EXISTS (SELECT 1 FROM blob_references`, was correctly absent — it is the owner's. | n/a (inspection) |
| `manifest_gc_releases_the_reference_for_an_unparseable_stored_principal` | The pre-fix inline `format!("{principal_id}:{manifest_digest}")` restored in place of the two-entry-point producer | **PASSED** (4 passed, 0 failed across `workspace::tests::manifest*`). **This test does not fail against the pre-fix code**, because the pre-fix inline format produced the identical composite and therefore released the reference correctly. The defect was the *duplication* of the format, not a wrong release. The test guards the refactor — that the new two-entry-point producer did not change the released owner id — and the "exactly one producer" property is enforced by the structural guard instead. Recorded as an unresolved finding. | yes |

The third row is the §13 requirement "how the digest-mismatch materializer case was shown to fail before the fix", satisfied. The fifth row is the §13 requirement "how the corrupt-principal owner-id case was shown to fail before the fix", which **could not be satisfied as written**; the honest result is recorded above and in [Unresolved findings](#unresolved-findings-by-severity).

## Verification actually run

Toolchain: repository-pinned Rust 1.89.0 (`rustc 1.89.0 (29483883e 2025-08-04)`, `cargo 1.89.0 (c24e10642 2025-06-23)`).
Host: Linux x86_64, kernel `6.8.0-147-generic`.
Build isolation: every cargo command ran with `CARGO_TARGET_DIR=/home/sugarwookie/projects/eggwork/target-w2b`, because a second agent is working in the same tree with a different target directory.

| Command | Result |
|---|---|
| `cargo check -p eggwork-server --lib` | exit 0, no warnings (final run, after the owner modules were wired up) |
| `cargo test -p eggwork-server --lib -- operations::tests` | final run: **22 passed, 0 failed**. Earlier runs in this milestone: a compile error (2 × `into_iter` on `&str`), then 20 passed / 2 failed from two fixture defects (a repeated `record_json` violating the artifacts `UNIQUE` constraint, and an `expired` blob that was seeded without its reference row and so counted as a candidate). Both were fixture bugs, fixed; no production code changed in response. |
| `cargo test -p eggwork-server --lib` | **142 passed, 0 failed, 0 ignored, 0 measured** |
| `cargo test -p eggwork-server --test gc_preview_parity` | **3 passed, 0 failed** |
| `cargo fmt -p eggwork-server` | applied; a second run after the clippy fixes produced no further change, and `cargo fmt -p eggwork-server -- --check` afterwards reported clean |
| `cargo clippy -p eggwork-server --all-targets -- -D warnings` | first run: **4 errors** (`doc_lazy_continuation`, 2 × `cloned_ref_to_slice_refs`, 1 × `bool_assert_comparison`) — all in the new test code. After fixing them: **exit 0, no diagnostics**. |
| `python3 scripts/check_execution_ownership.py` | passed: "execution ownership guard passed; approved process owners: eggwork-runner (2 spawn sites), eggwork-sandbox-helper (1 spawn sites)"; "negative scanner self-test passed; crate dependency direction passed" |
| `git diff --check` | clean, no output |

**Commands deliberately not run, and why:**

- `cargo test --locked --workspace --all-targets` and `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` — the coordinator reserved the full gate for the end of the batch, and a second agent is concurrently editing `crates/eggwork-runner/**` and `crates/eggwork-sandbox-helper/**` in this tree. **Acceptance criterion 13 is therefore not discharged by this record.**
- `cargo fmt --all` — the instruction for this milestone was to format only `eggwork-server`, because the other agent's crates are in flight.
- `cargo test --locked -p eggwork-server --test installed_qualification` — it is `#[ignore]`d and opt-in; it needs an installed daemon and TLS material. **Not exercised.**
- `python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release` — **not exercised.** No Python test was added under `tests/release/`: the structural guard reads Rust source, so it belongs in a Rust integration test (`crates/eggwork-server/tests/gc_preview_parity.rs`) rather than in the release-harness suite. The release-harness suite is consequently unaffected by this milestone.
- `crates/eggwork-server/tests/release_contract.rs` and `crates/eggwork-server/tests/host_telemetry.rs` — **not executed** by this milestone (the scoped instruction named the lib suite and this milestone's own new test file).

**A note on `--locked`.** The scoped commands above were run *without* `--locked`. This milestone changed no dependency and no `Cargo.lock` entry, so the flag would not have changed the resolution; the omission is recorded here rather than glossed over, because `AGENTS.md` treats `--locked` as load-bearing on `clippy` and `test`.

## Platform and feature matrix actually exercised

| Target | Exercised? | Evidence |
|---|---|---|
| Linux x86_64 (kernel 6.8.0-147-generic) | **Yes** | The full scoped suite: `cargo test -p eggwork-server --lib` → 142 passed, 0 failed; `cargo test -p eggwork-server --test gc_preview_parity` → 3 passed. This includes the unix-only branches: the `0600`/`0700` permission helpers, `symlink_metadata`-based symlink refusal in `inspect_blob`, and the unix-only `inspect_blob_refuses_a_symlinked_shard_or_object` test. |
| Linux on other architectures (aarch64, etc.) | **No** | Not executed. Nothing here qualifies any architecture other than the one above. |
| macOS | **No** | Not executed, and not qualified by this record. |
| Windows | **No** | Not executed, and not qualified by this record. The `#[cfg(not(unix))]` permission helpers and the `sync_directory` no-op were **not** exercised; neither was any `#[cfg(windows)]` branch. The one Windows-relevant code path this milestone touches is the staged-file removal on a failed copy: the staged file is closed before it is removed, so the removal is not attempted against an open handle — but that ordering was **not** exercised on Windows, and it is listed as an unresolved finding. |
| The `qualification` feature / installed-release harness | **No** | Not built, not exercised. |
| The runner and sandbox helper crates | **No** | Out of scope for this milestone and concurrently edited by another agent. `scripts/check_execution_ownership.py` passed, which confirms this milestone added no process creation to `eggwork-server`. |

**No platform is claimed to be qualified by this record.** The milestone's changes are platform-neutral in shape, but only the row marked "Yes" was executed.

## Compatibility review

**Stored formats — unchanged.** No `CREATE TABLE` statement was altered in `blob.rs`, `workspace.rs` or `artifact.rs`; no column, index, trigger or default was added, removed or renamed. The blob shard layout is still `<root>/<first two hex characters>/<64-hex digest>` — `blob::object_path` computes the same path the old inline expression computed, and `BlobStore::path_for` delegates to it, so the bytes on disk are identical. The three metadata file names are unchanged (`metadata.sqlite`, `metadata.sqlite`, `artifacts.sqlite`); only the *place they are spelled* moved. The `"{principal}:{manifest_digest}"` composite is byte-identical. The manifest digest algorithm, the artifact record format, the workspace record format and the journal schema are untouched.

**Wire and protocol surfaces — unchanged.** No route, request struct, response struct, capability string, status code or error body was added, removed or altered. `NodeGcReport` keeps the same thirteen fields, names, types and order, and `dry_run` is still set by the caller. `ResourceInspection` keeps the same three fields, and the `metadata` JSON each `inspect` verb emits is unchanged in shape and content. The digest mismatch in the materializer is surfaced as the **existing** `WorkspaceError::InvalidBlob`, which `lib.rs` already maps to `409 missing_blob` — the same code a missing blob produces. No new error code was introduced.

**Bounds — unchanged.** `MAX_GC_BATCH` (1024) and the `1..=1024` clamp, `MAX_BLOB_BYTES`, `MAX_WORKSPACE_ENTRIES`, `MAX_ARTIFACT_FILES`, `MAX_ARTIFACT_TOTAL_BYTES`, the workspace and blob quotas, and the retention horizons are all as they were. The bounded-per-pass semantics are unchanged: one invocation removes at most `limit` items per store. The GC ordering — workspaces → manifests → artifacts → blobs — is unchanged in both the operator path and `NodeServer::collect_garbage`. The apply pass's exclusive maintenance lock, and its refusal to race a running node, are unchanged.

**Authentication, authorization, leases, generations, fencing, digest bounds — unchanged.** None of that code was touched. The artifact-capture eligibility rule (M005a's concern), the journal's event retention, and the recovery classification are untouched.

**What did change for the operator.**

1. **`eggworkd gc` (preview) may now report different blob figures than before.** The preview used to count blobs with `reference_count = 0`; it now reports the store's own candidate set — blobs with no reference row. On a store whose trigger-maintained counter has drifted from its reference rows, the two disagree; on a consistent store they agree. The reported *meaning* is the correct one: the preview now answers the question the apply pass will act on.
2. **A workspace whose manifest references a blob whose bytes do not match its digest is now refused** (`409 missing_blob`) instead of being materialized from the mismatched bytes when the length happened to match. This is a behaviour change on a corrupted store: the execution fails closed instead of running against the wrong bytes. The wire code is pre-existing; no new code appears on the wire.
3. **`eggworkd gc` (preview) against a store that does not exist** still fails with the generic "bounded garbage collection failed" error, exactly as before — but the *reason* is now a guarantee rather than an accident of `SQLITE_OPEN_READ_ONLY` on a missing file, and it is covered by a test.
4. **`eggworkd inspect blob`** resolves the object through the store's path function. Its observable behaviour — the refusal of a symlinked shard or object, and the recorded-size-versus-file-length cross-check — is unchanged.

**Performance.** One SHA-256 computation over the copied bytes per materialized file, and no additional read or byte movement. Detailed in [Verify-while-copying, not verify-then-copy](#verify-while-copying-not-verify-then-copy).

## Documentation status

- **`architecture/data-plane.md` — updated.** The single-owner facts (the metadata file names, the blob shard layout, the manifest owner-id composite, and the four canonical candidate statements); the non-mutating preview seam and what it costs; the reference-row predicate and the deliberate reason the counter is not consulted; the preview-versus-apply relation (subset, and equality in the no-expired-references case); the verify-while-copying change and its performance consequence; and new rows in the invariants-and-enforcement table. **All new text uses symbol references, not line numbers** — `UNREFERENCED_CANDIDATES_SQL`, `blob::preview_gc`, `BlobStore::copy_verified`, `manifest_owner_id_for_key`, and so on. The document's pre-existing stale line-number citations were **left as they are**: replacing them wholesale is a separate, much larger correction and is out of this milestone's scope, but no new line-number citation was added.
- **`architecture/operations-cli.md` — updated.** The `gc` verb reference now describes the preview as obtaining its candidate sets from the owner modules and holding no SQL of its own; the preview's non-creating behaviour; the preview-versus-apply relation and the divergence it removes; and `inspect blob`'s path resolution. The artifact database path is now described as `artifact::metadata_path(database_path)`. Two rows were added to the invariants table. New text uses symbol references.
- **`docs/operations.md` — updated.** Two operator-facing paragraphs in the "Garbage collection" section: the preview never creates or reconciles a store, and the preview may report fewer candidates than the apply pass removes for a documented reason. No other operator-facing behaviour changed, so no other section was touched.
- **`architecture/overview.md` — NOT edited.** Coordinator-owned. The text that belongs there is reported to the coordinator rather than written.
- **`plans/registry.md` — NOT edited.** Coordinator-owned; see [Registry and roadmap disposition](#registry-and-roadmap-disposition).

## Disposition of the in-process collector path

`NodeServer::collect_garbage` is **retained as-is**. It already calls the owner collectors, and after this milestone the owner collectors, the operator's preview, and the in-process path all evaluate the same module constants (`UNREFERENCED_CANDIDATES_SQL`, `WORKSPACE_CANDIDATES_SQL`, `MANIFEST_CANDIDATES_SQL`, `ARTIFACT_CANDIDATES_SQL`), so the in-process path and the operator's path cannot disagree about what is collectable.

The two entry points are not *merged*, for a reason that is not about the predicate. The operator path's defining property is that it takes the exclusive maintenance lock and therefore refuses to run against a live node; the in-process path's defining property is that it runs inside the node against the already-open store instances. Merging them would give up one of those two properties. The in-process path is not exposed as a remote route.

## Unresolved findings by severity

| Severity | Finding | Owner |
|---|---|---|
| **Medium** | The workspace-wide gate of acceptance criterion 13 (`cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`, `cargo test --locked --workspace --all-targets`) was not run by this milestone. The scoped equivalents pass, and a second agent is concurrently editing the runner and the sandbox helper in this tree, so a workspace-wide run would have raced that work. | **Coordinator** — run the full gate once at the end of the batch. |
| **Medium** | The plan's section 13 expects the corrupt-principal owner-id case to be "shown to fail before the fix". It does not fail before the fix, and this was verified experimentally rather than assumed: the pre-fix inline `format!` produced the identical composite, so the pre-fix code released the reference correctly. The defect was the duplication of the format, not a wrong release. The test as written is a refactor-regression guard; the "exactly one producer" property is enforced by the structural guard instead. The plan's expectation should be corrected, or a different test should be specified. | **Coordinator** — the plan's section 13 text; the implementing agent cannot amend `plans/implementation/**`. |
| **Low** | The materializer removes the staged file after a failed copy. The staged file is closed before the removal, so the removal is not attempted against an open handle — but this ordering was **not** exercised on Windows, where removing an open file fails. If the removal fails on Windows, the unverified bytes stay in the staging directory, which `materialize` then removes wholesale on the error path; the tree is still never published. | **Coordinator / future Windows milestone** — the Windows service host (Operations M007) is the natural place to exercise the Windows materialization path. |
| **Low** | `workspace::preview_gc` reads `sqlite_master` to decide whether the `retained_manifests` table exists, so that a pre-M004 store reports zero manifest candidates instead of failing the whole preview. This is a read of SQLite's catalogue, not a schema change, but it is a second place that knows the table's name. | **Coordinator** — acceptable as-is; would need revisiting if the table name ever changes. |
| **Informational** | The operator module still reads a single record by id for the three `inspect` verbs (`blobs`, `workspaces`, `artifacts`), so it still knows those tables' column names. Moving those reads into the owner modules was considered and deliberately **not** done: the plan's section 5.3 scopes the blob-ownership change to the *path*, and the inspection verbs legitimately need a record projection. The structural guard is scoped accordingly — it forbids the reclamation predicates, not SQL in general. | **Coordinator** — a future data-plane milestone may want to move the inspection reads as well. |

## Registry and roadmap disposition

- **`plans/subsystems/workspace-artifact-transport-roadmap.md` — updated by this milestone.** The status line now reads *"Status: active roadmap; M001-M004 closed; M005a data-plane corrective proposed; M005b data-plane corrective implemented"*, and the M005b entry's status changed from `proposed` to `implemented`, with a `Closure record:` block pointing at this record and a short `Implemented:` summary in the density of the neighbouring entries. **The M005a section was not touched**, and no other roadmap's status line was touched.
- **`plans/registry.md` — NOT edited by this milestone.** The registry is owned by the planning process, not by the implementing agent (plan section 14). The coordinator owns the entry that points at this closure record and at the M005b milestone.
- **Both closure records are preserved.** M005a's record (if the coordinator has written it) and this one stand side by side; nothing in the historical M001-M004 records was rewritten.

## Deviations from the plan

| Deviation | Reason |
|---|---|
| The materializer uses a **new** store-owned routine, `BlobStore::copy_verified`, rather than the roadmap's exit-condition wording "the workspace materializer uses the store's verified-open path" (`BlobStore::open_verified`). | `open_verified` verifies the object and then opens it — two reads of a mutable file, with a window between them. The plan's section 5.4 requires the verification to be *"bound to the bytes actually copied"*, which `open_verified` cannot provide. `copy_verified` is store-owned and satisfies section 5.4 directly. The roadmap's exit-condition wording is now slightly behind the implementation; the coordinator may wish to align it. |
| The workspace-side candidate helpers are named `workspace_candidates` and `manifest_candidates` (module-private) rather than being exposed as `pub`. | Only the *preview* entry points need to be public. The collectors call the same module-private helpers inside the owning module, which is where the predicate has to live. Exposing more would widen the surface without widening the guarantee. |
| The artifact collector's redundant unbounded `COUNT(*)` of expired artifacts was removed, and the delete now removes the rows the candidate window named, one statement per artifact id. | The unbounded count was the same predicate written a second time in the owner module, and `candidate_count.min(count)` was always `candidate_count` because the window is a bounded subset of the same set — so the reported values are unchanged. Removing it leaves exactly one implementation of the artifact predicate. The delete shape changed (parameterized per-id deletes instead of one `IN (subquery)`); the removed set is identical. |
| The manifest GC validates the row's principal where it can and falls back to the raw key otherwise, rather than always using the raw key. | The plan's section 5.5 asked for the validated entry point where validation is possible. The fallback is retained because one call site is reached precisely when the principal does not parse, and refusing to release the reference there would leak the pin. Both forms produce the same bytes, so the released owner id is unchanged. |
| The corrupt-principal test does not reproduce a pre-fix failure. | See [Unresolved findings](#unresolved-findings-by-severity). The plan's expectation was that it would; it does not, and this was verified rather than assumed. |
| No Python test was added under `tests/release/`. | The structural guard reads Rust source, so it belongs in a Rust integration test. `tests/release/test_gc_preview_parity.py` was not created, and the release-harness suite is unaffected. |
| The scoped cargo commands were run without `--locked`. | Recorded above. This milestone changed no dependency and no `Cargo.lock` entry. |

## Change set

| File | Change |
|---|---|
| `crates/eggwork-server/src/blob.rs` | The canonical blob candidate statement and the expired-reference count as module constants; `BlobStore::unreferenced_candidates` takes a `&Connection`; `blob::preview_gc`, `blob::metadata_path`, `blob::shard_path`, `blob::object_path`; `BlobStore::path_for` delegates to `object_path`; `BlobStore::copy_verified` and the `VerifyWriter` hashing writer; `remove_stale_parts` resolves through the owned path function. |
| `crates/eggwork-server/src/workspace.rs` | The canonical workspace and retained-manifest statements; `workspace::preview_gc`, `workspace::metadata_path`, `workspace_candidates`, `manifest_candidates`, `table_exists`; `manifest_owner_id_for_key` / `manifest_owner_id` and the two former inline call sites routed through them; `materialize_tree` verifies the copied bytes through `BlobStore::copy_verified` and removes the staged file on failure. |
| `crates/eggwork-server/src/artifact.rs` | The canonical artifact candidate statement; `artifact::preview_gc`, `artifact::metadata_path`, `artifact_candidates`; the collector selects through the canonical statement and deletes the rows it named. |
| `crates/eggwork-server/src/operations.rs` | `dry_run_gc` obtains its candidate sets from the owner modules and holds no SQL; `inspect_blob` resolves the shard and object through the store's path ownership; `inspect_workspace`, `inspect_artifact` and `storage_summary` resolve the metadata path through the owner; the new tests. |
| `crates/eggwork-server/src/lib.rs` | The artifact database path now comes from `artifact::metadata_path`. |
| `crates/eggwork-server/tests/gc_preview_parity.rs` | **New.** The structural guard. |
| `architecture/data-plane.md` | The single-owner facts, the preview seam, the reference-row predicate, the verify-while-copying consequence, the new invariants rows, the new test coverage, the honest gaps. |
| `architecture/operations-cli.md` | The `gc` verb reference and the `inspect blob` reference; the new invariants rows. |
| `docs/operations.md` | Two operator-facing paragraphs in the GC section. |
| `plans/subsystems/workspace-artifact-transport-roadmap.md` | The status line, the M005b status, the closure-record pointer, the implemented summary. |
| `plans/closure/workspace-artifact-transport/005b-status.md` | **New.** This record. |

No file outside this list was modified by this milestone. `crates/eggwork-core/**`, `crates/eggwork-client/**`, `crates/eggwork-runner/**`, `crates/eggwork-sandbox-helper/**`, `crates/eggwork-server/src/store.rs`, `scripts/**`, `tests/release/**`, `architecture/overview.md`, `plans/registry.md`, `plans/000-*` through `plans/003-*`, and every `Cargo.toml` were left alone.
