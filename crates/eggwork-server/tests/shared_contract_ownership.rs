//! Each shared protocol contract fact has exactly one owner.
//!
//! Three facts used to be stated by more than one owner, and the copies had
//! no mechanical relationship: the control-plane schema version, the
//! `exec.argv.v1` capability name, and the terminal/non-terminal
//! classification of `ExecutionState`. Both sides of every pair agreed by
//! value, so none of it was a live bug — the defect was structural, because a
//! future rename or bump on the producing side would have desynchronised the
//! consuming side silently.
//!
//! The behavioural tests prove the values agree today. These tests prove the
//! *shape* of the ownership: that the copies are gone. A private copy
//! reintroduced in an endpoint crate would leave every behavioural test
//! green — the copy would hold the same value — and would fail here.
//!
//! The scan is a source check rather than a behavioural one on purpose: there
//! is no way to observe "a second declaration exists" from outside the
//! crate, and a private copy that happens to hold the right value is exactly
//! the thing that has to be caught.

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the server crate lives inside the workspace")
        .to_path_buf()
}

fn read(relative: &str) -> String {
    let path = workspace_root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

/// The production part of a source file.
///
/// Everything before the first `#[cfg(test)]` item. In the endpoint crates the
/// test modules are all at the end of the file, so this is exactly the
/// production code. A literal that only appears in a test module is not a
/// second *production* copy of a contract fact, so the scan ignores it — the
/// tests are allowed to name the values they assert.
fn production_source(relative: &str) -> String {
    let source = read(relative);
    match source.find("#[cfg(test)]") {
        Some(index) => source[..index].to_owned(),
        None => source,
    }
}

/// The number of occurrences of the given needle in the given text.
fn occurrences(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

/// The schema version is declared once in the workspace, in `eggwork-core`.
///
/// This is the check that fails if a private copy is reintroduced in the
/// client or the server: the second `const API_SCHEMA_VERSION` declaration
/// would make this count exceed one.
#[test]
fn the_schema_version_is_declared_exactly_once_in_the_workspace() {
    let mut declarations: Vec<(String, usize)> = Vec::new();
    for relative in [
        "crates/eggwork-core/src/lib.rs",
        "crates/eggwork-client/src/lib.rs",
        "crates/eggwork-server/src/lib.rs",
        "crates/eggwork-runner/src/lib.rs",
        "crates/eggwork-sandbox-helper/src/main.rs",
    ] {
        let source = read(relative);
        let count = occurrences(&source, "const API_SCHEMA_VERSION");
        if count > 0 {
            declarations.push((relative.to_owned(), count));
        }
    }
    assert_eq!(
        declarations,
        vec![("crates/eggwork-core/src/lib.rs".to_owned(), 1)],
        "the schema version must be declared once, publicly, in eggwork-core"
    );

    // The core's declaration must be public, or the endpoint crates could not
    // import it and would have to keep a copy.
    let core = read("crates/eggwork-core/src/lib.rs");
    assert!(
        core.contains("pub const API_SCHEMA_VERSION: u16 = 1;"),
        "the core's schema version must be public and frozen at 1"
    );
}

/// Neither endpoint crate re-types the argv capability name.
///
/// The server produces the name and the client consumes it, so a literal in
/// either production file is a second copy waiting to drift.
#[test]
fn the_argv_capability_name_is_not_retyped_in_either_endpoint_crate() {
    for relative in [
        "crates/eggwork-client/src/lib.rs",
        "crates/eggwork-server/src/lib.rs",
    ] {
        let source = production_source(relative);
        assert_eq!(
            occurrences(&source, "\"exec.argv.v1\""),
            0,
            "{relative} must import the owned capability name, not re-type it"
        );
    }
}

/// The server does not re-type the runner's cgroup feature names.
///
/// The runner produces them; the server only consumes them.
#[test]
fn the_cgroup_feature_names_are_not_retyped_in_the_server() {
    let source = production_source("crates/eggwork-server/src/lib.rs");
    for literal in [
        "\"resources.cgroups-v2.memory\"",
        "\"resources.cgroups-v2.cpu\"",
        "\"resources.cgroups-v2.pids\"",
    ] {
        assert_eq!(
            occurrences(&source, literal),
            0,
            "the server must import the runner's exported constant, not re-type {literal}"
        );
    }
}

/// Each capability feature literal is first declared in the constant that owns
/// it.
///
/// The runner's source has test modules in the middle of the file, so the
/// check is "the first occurrence of the literal is the owning constant's
/// declaration" rather than "the literal occurs once". A literal typed back
/// into the advertisement site — which sits *above* the constants — would
/// become the first occurrence and fail this.
#[test]
fn the_runner_declares_each_capability_literal_in_the_constant_that_owns_it() {
    let source = read("crates/eggwork-runner/src/lib.rs");
    let lines: Vec<&str> = source.lines().collect();
    for (constant, literal) in [
        (
            "pub const LANDLOCK_WORKSPACE_RW_CAPABILITY",
            "\"isolation.landlock.workspace-rw.v1\"",
        ),
        (
            "pub const CGROUPS_V2_MEMORY_CAPABILITY",
            "\"resources.cgroups-v2.memory\"",
        ),
        (
            "pub const CGROUPS_V2_CPU_CAPABILITY",
            "\"resources.cgroups-v2.cpu\"",
        ),
        (
            "pub const CGROUPS_V2_PIDS_CAPABILITY",
            "\"resources.cgroups-v2.pids\"",
        ),
        ("pub const WORKSPACE_RW_PROFILE", "\"workspace_rw\""),
    ] {
        let literal_line = lines
            .iter()
            .position(|line| line.contains(literal))
            .unwrap_or_else(|| panic!("{literal} is not declared in the runner"));
        let constant_line = lines
            .iter()
            .position(|line| line.contains(constant))
            .unwrap_or_else(|| panic!("{constant} is not declared in the runner"));
        assert_eq!(
            literal_line, constant_line,
            "{literal} must first appear in {constant}, not at an earlier site"
        );
    }
}

/// The server's production code names no execution state as a string.
///
/// The state vocabulary is derived from the core-owned wire spelling: the
/// store writes it, the store reads it back, and the operator query filters
/// on it. A state name written as a literal in any of these files would be a
/// copy that the shared classification no longer governs.
#[test]
fn the_server_names_no_execution_state_as_a_string_literal() {
    for relative in [
        "crates/eggwork-server/src/lib.rs",
        "crates/eggwork-server/src/store.rs",
        "crates/eggwork-server/src/operations.rs",
    ] {
        let source = production_source(relative);
        for name in [
            "Accepted",
            "Preparing",
            "Running",
            "Cancelling",
            "Succeeded",
            "Failed",
            "Cancelled",
            "TimedOut",
            "Interrupted",
        ] {
            assert_eq!(
                occurrences(&source, &format!("\"{name}\"")),
                0,
                "{relative} must not name the state {name} as a string literal"
            );
            assert_eq!(
                occurrences(&source, &format!("'{name}'")),
                0,
                "{relative} must not name the state {name} as a SQL literal"
            );
        }
    }
}

/// The core's `ExecutionState` keeps its derived wire representation.
///
/// A `#[serde(rename_all = ...)]` attribute on the enum would move every
/// persisted and transmitted state. The attribute must not be there.
#[test]
fn the_core_state_enum_keeps_its_derived_wire_representation() {
    let source = read("crates/eggwork-core/src/lib.rs");
    let enum_start = source
        .find("pub enum ExecutionState")
        .expect("the enum is declared in the core crate");
    let declaration = &source[enum_start..];
    let end = declaration.find("\n}").expect("the enum declaration ends");
    let declaration = &declaration[..end];
    assert!(
        !declaration.contains("rename"),
        "the state enum must keep its derived Serialize/Deserialize representation"
    );
    assert!(
        !declaration.contains("serde("),
        "the state enum must keep its derived Serialize/Deserialize representation"
    );
}
