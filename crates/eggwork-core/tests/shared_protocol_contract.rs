//! The protocol contract facts `eggwork-core` owns.
//!
//! Before this milestone the control-plane schema version, the `exec.argv.v1`
//! capability name and the terminal/non-terminal classification of
//! `ExecutionState` were each stated by more than one owner, and the copies
//! had no mechanical relationship: both sides of every pair happened to agree
//! by value, so a future rename or bump on the producing side would have
//! desynchronised the consuming side silently.
//!
//! These tests are the mechanical half of the fix. They pin the *values* (so a
//! careless edit is a test failure rather than a protocol break), they pin the
//! *classification* (so the two sets cannot drift or lose a member), and they
//! pin the wire spelling to the derived `Serialize` (so the journal's `state`
//! column stays honest against the enum).
//!
//! **Adding a tenth variant to `ExecutionState` breaks this suite.** The
//! primary mechanism is structural: `ExecutionState::is_terminal`,
//! `ExecutionState::is_non_terminal` and `ExecutionState::wire_name` are
//! exhaustive `match`es over all nine variants with no wildcard arm, so a new
//! variant does not compile in `eggwork-core` at all. The secondary mechanism
//! is `the_execution_state_enum_has_exactly_nine_variants`, which fails if the
//! enumeration constant is widened without the classification being updated.
//! The property test ranges over the crate-owned
//! `eggwork_core::ALL_EXECUTION_STATES`, never over a list written here, so it
//! cannot be satisfied by a hand-written list of nine variants that a tenth
//! one simply joins unnoticed.

use eggwork_core::{
    ALL_EXECUTION_STATES, API_SCHEMA_VERSION, EXEC_ARGV_CAPABILITY, ExecutionState,
};

/// The control-plane schema version is frozen at 1.
///
/// The value is a wire fact: a client stamps it into every request body and a
/// server refuses a body that does not carry it. Changing it is a
/// compatibility break, not a refactor.
#[test]
fn the_control_plane_schema_version_is_frozen_at_one() {
    assert_eq!(API_SCHEMA_VERSION, 1);
}

/// The argv capability feature name is frozen at `exec.argv.v1`.
///
/// A node advertises this name and a client demands it before it will submit
/// an execution, so the bytes are a contract between two crates.
#[test]
fn the_exec_argv_capability_name_is_frozen() {
    assert_eq!(EXEC_ARGV_CAPABILITY, "exec.argv.v1");
}

/// The state enum still has exactly nine variants.
///
/// This is the second half of the "adding a variant breaks the suite"
/// guarantee: the exhaustive matches in `eggwork-core` stop the enum from
/// growing silently, and this assertion stops the enumeration constant from
/// growing silently either.
#[test]
fn the_execution_state_enum_has_exactly_nine_variants() {
    assert_eq!(
        ALL_EXECUTION_STATES.len(),
        9,
        "the shared state enumeration must be widened together with the classification"
    );
}

/// Every variant lands in exactly one of the two sets.
///
/// A state that is neither terminal nor non-terminal — or both — would make
/// every consumer's answer depend on which arm it happened to check.
#[test]
fn every_state_is_classified_into_exactly_one_set() {
    for state in &ALL_EXECUTION_STATES {
        assert_ne!(
            state.is_terminal(),
            state.is_non_terminal(),
            "{state:?} is classified as both terminal and non-terminal, or as neither"
        );
    }
}

/// The terminal set has five members and the non-terminal set four.
///
/// The membership is spelled out, not just the counts: a mis-edited
/// classification — `Failed` flipped to non-terminal, say — still partitions
/// the enum, and only the membership catches it.
#[test]
fn the_terminal_set_has_five_members_and_the_non_terminal_set_four() {
    let terminal: Vec<&str> = ALL_EXECUTION_STATES
        .iter()
        .filter(|state| state.is_terminal())
        .map(|state| state.wire_name())
        .collect();
    let non_terminal: Vec<&str> = ALL_EXECUTION_STATES
        .iter()
        .filter(|state| state.is_non_terminal())
        .map(|state| state.wire_name())
        .collect();

    assert_eq!(
        terminal,
        [
            "Succeeded",
            "Failed",
            "Cancelled",
            "TimedOut",
            "Interrupted"
        ]
    );
    assert_eq!(
        non_terminal,
        ["Accepted", "Preparing", "Running", "Cancelling"]
    );
    assert_eq!(terminal.len(), 5);
    assert_eq!(non_terminal.len(), 4);
    assert_eq!(
        terminal.len() + non_terminal.len(),
        ALL_EXECUTION_STATES.len()
    );
}

/// The canonical wire spelling is exactly what the derived `Serialize` emits.
///
/// This is what keeps the journal's `state` column honest against the enum:
/// the column stores the PascalCase spelling, so the spelling has to be the
/// enum's own serialised form and not a second, hand-maintained list.
#[test]
fn the_wire_spelling_matches_the_derived_serde_representation() {
    for state in &ALL_EXECUTION_STATES {
        let encoded = serde_json::to_string(state).expect("a state must encode");
        assert_eq!(
            encoded,
            format!("\"{}\"", state.wire_name()),
            "{state:?} has a wire spelling that disagrees with its serde form"
        );
        let decoded: ExecutionState = serde_json::from_str(&encoded).expect("a state must decode");
        assert_eq!(&decoded, state);
    }
}

/// The wire form is still externally tagged PascalCase with no rename.
///
/// The enum's `Serialize`/`Deserialize` derives are untouched: a state encodes
/// as the bare string `"TimedOut"`, not as a tagged object, and not as
/// `"timed_out"`. A rename attribute here would silently move every persisted
/// state.
#[test]
fn the_state_wire_form_is_still_a_bare_pascal_case_string() {
    assert_eq!(
        serde_json::to_string(&ExecutionState::TimedOut).unwrap(),
        "\"TimedOut\""
    );
    assert_eq!(
        serde_json::to_string(&ExecutionState::Accepted).unwrap(),
        "\"Accepted\""
    );
    assert_eq!(
        serde_json::from_str::<ExecutionState>("\"TimedOut\"").unwrap(),
        ExecutionState::TimedOut
    );
}
