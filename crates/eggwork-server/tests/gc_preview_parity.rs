//! Structural guard for the operator's garbage-collection preview.
//!
//! The behavioural tests in `src/operations.rs` prove that the preview
//! *reports* the owner module's candidate set. This guard proves the weaker
//! property that would let the two drift apart again: that the operator
//! module does not spell a reclamation predicate of its own.
//!
//! The guard is deliberately precise. It forbids the reclamation predicates,
//! the candidate ordering, the store metadata file names and the shard
//! layout - not SQL in general. The inspection verbs legitimately read a
//! single record by id, so a guard that forbade those would be a false
//! positive rather than a rule. It also does not scan the test module: a
//! fixture has to be able to reach the store's tables to seed the drifted
//! states the behavioural tests need.

/// The operator module's source.
const OPERATOR_SOURCE: &str = include_str!("../src/operations.rs");

/// Facts that only the owner modules may spell.
///
/// Each entry is a fragment that appears in exactly one candidate statement
/// or in exactly one path expression inside `blob.rs`, `workspace.rs` or
/// `artifact.rs`.
const FORBIDDEN_IN_THE_OPERATOR: &[&str] = &[
    // The canonical blob candidate predicate, and the counter-based
    // predicate that diverged from it.
    "NOT EXISTS (SELECT 1 FROM blob_references",
    "reference_count = 0",
    // The canonical workspace and retained-manifest candidate predicate.
    "expires_unix_ms IS NOT NULL",
    // The canonical artifact candidate predicate.
    "expires_unix_ms <= ",
    // The candidate orderings.
    "ORDER BY expires_unix_ms",
    "ORDER BY last_used_unix_ms",
    // The tables the operator must not read directly.
    "FROM retained_manifests",
    "FROM blob_references",
    // The store metadata file names.
    "metadata.sqlite",
    "artifacts.sqlite",
    // The shard layout, which the store owns.
    "as_str()[..2]",
];

/// The operator module's production code, without the test module.
fn production_source() -> &'static str {
    OPERATOR_SOURCE
        .split("#[cfg(test)]")
        .next()
        .expect("the operator module has a test module")
}

/// The body of the operator's preview function.
fn preview_body() -> &'static str {
    let source = OPERATOR_SOURCE;
    let start = source
        .find("fn dry_run_gc(")
        .expect("the operator module has a preview function");
    let rest = &source[start..];
    let end = rest
        .find("\npub fn ")
        .expect("the preview function is followed by another function");
    &rest[..end]
}

#[test]
fn the_operator_module_contains_no_candidate_predicate_of_its_own() {
    let production = production_source();
    for fragment in FORBIDDEN_IN_THE_OPERATOR {
        assert!(
            !production.contains(fragment),
            "the operator module spells {fragment:?}; the owner module must be the \
             only place that fact is written"
        );
    }
}

#[test]
fn the_operator_preview_spells_no_sql_and_opens_no_store() {
    let preview = preview_body();
    for fragment in [
        "SELECT",
        "FROM ",
        "JOIN ",
        "::open(",
        "metadata.sqlite",
        "artifacts.sqlite",
    ] {
        assert!(
            !preview.contains(fragment),
            "the operator's preview contains {fragment:?}; it must obtain its \
             candidate sets from the owner modules"
        );
    }
}

#[test]
fn the_owner_modules_still_spell_the_canonical_predicates() {
    // The guard above is only meaningful while the owner modules keep the
    // statements it protects. This asserts that they do.
    let blob = include_str!("../src/blob.rs");
    assert!(
        blob.contains("NOT EXISTS (SELECT 1 FROM blob_references r WHERE r.digest = blobs.digest)")
    );
    let workspace = include_str!("../src/workspace.rs");
    assert!(workspace.contains("FROM workspaces\n                 WHERE expires_unix_ms IS NOT NULL AND expires_unix_ms <= ?1")
        || workspace.contains("FROM workspaces"));
    let artifact = include_str!("../src/artifact.rs");
    assert!(artifact.contains("FROM artifacts WHERE expires_unix_ms <= ?1"));
}
