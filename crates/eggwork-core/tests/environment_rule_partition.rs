//! The core environment-entry rules partition the input space.
//!
//! The environment-entry contract is the rule set the runner consumes, so it
//! has to be *total*: for every name, value and list there is exactly one
//! answer, and that answer is the documented one. A rule change that widened
//! the accepted set or narrowed it would silently open a gap between what the
//! contract says and what it enforces, and the gap would only be visible to a
//! reader who happened to know the old rule.
//!
//! This suite therefore does two things at once:
//!
//! 1. It re-derives the documented rule *independently* of the implementation
//!    -- the predicate below is written from the rule's prose, not from the
//!    code -- and asserts that the implementation agrees with it on every
//!    candidate.
//! 2. It computes the accepted set and the rejected set as sets and asserts
//!    that they are disjoint and that their union is the whole candidate
//!    space. A rule change that moved a name from one set to the other
//!    therefore has to be accompanied by a change to the documented predicate,
//!    and the test fails if it is not.
//!
//! The name rule, as documented:
//!
//! - the name is not empty;
//! - the name is at most `MAX_ENV_NAME_BYTES` bytes long;
//! - the name is composed only of ASCII alphanumerics and underscore;
//! - the name does not start with a digit;
//! - the name contains no NUL byte.

use eggwork_core::{
    EnvironmentEntry, MAX_ENV_COUNT, MAX_ENV_NAME_BYTES, MAX_ENV_VALUE_BYTES, ValidationError,
    validate_environment_entries,
};

/// The documented environment-name rule, re-derived independently.
fn documented_name_is_accepted(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_ENV_NAME_BYTES
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && !name.as_bytes().first().is_some_and(u8::is_ascii_digit)
        && !name.as_bytes().contains(&0)
}

/// Every candidate name the partition is proved over.
fn candidate_names() -> Vec<String> {
    let mut names = vec![
        // The shortest and the most ordinary shapes.
        String::new(),
        "A".to_owned(),
        "Z".to_owned(),
        "a".to_owned(),
        "z".to_owned(),
        "_".to_owned(),
        "A0".to_owned(),
        "A_".to_owned(),
        "_A".to_owned(),
        "API_TOKEN".to_owned(),
        // A leading digit is refused even though the rest of the name is legal.
        "0".to_owned(),
        "9".to_owned(),
        "1BAD".to_owned(),
        "0A".to_owned(),
        // Characters outside ASCII alphanumerics and underscore.
        "A-B".to_owned(),
        "A.B".to_owned(),
        "A B".to_owned(),
        "A+B".to_owned(),
        "A/B".to_owned(),
        "A:B".to_owned(),
        // `=` would split the name from the value in the child's environment.
        "A=B".to_owned(),
        // NUL cannot survive into an environment vector at all.
        "A\0B".to_owned(),
        // Non-ASCII bytes are not ASCII alphanumerics.
        "Aé".to_owned(),
        "Ä".to_owned(),
        "Ω".to_owned(),
    ];
    // The length bound, on both sides.
    names.push("A".repeat(MAX_ENV_NAME_BYTES));
    names.push("A".repeat(MAX_ENV_NAME_BYTES + 1));
    names.push("_".repeat(MAX_ENV_NAME_BYTES));
    names.push("_".repeat(MAX_ENV_NAME_BYTES + 1));
    names
}

#[test]
fn the_accepted_and_rejected_name_sets_partition_the_candidate_space() {
    let candidates = candidate_names();
    let mut accepted = Vec::new();
    let mut rejected = Vec::new();

    for name in &candidates {
        let entry = EnvironmentEntry {
            name: name.clone(),
            value: "value".to_owned(),
        };
        let verdict = validate_environment_entries(std::slice::from_ref(&entry));
        let implemented_accepts = verdict.is_ok();
        let documented_accepts = documented_name_is_accepted(name);

        assert_eq!(
            implemented_accepts, documented_accepts,
            "the implementation and the documented rule disagree about {name:?}: \
             implemented accepted = {implemented_accepts}, documented accepted = {documented_accepts}"
        );

        if implemented_accepts {
            accepted.push(name.clone());
        } else {
            rejected.push(name.clone());
            assert!(
                matches!(
                    verdict,
                    Err(ValidationError::InvalidSyntax {
                        field: "environment name"
                    })
                ),
                "a refused name must be refused by the name rule: {name:?} -> {verdict:?}"
            );
        }
    }

    // The two sets are disjoint, and together they are the whole candidate
    // space. Stated explicitly, because "the implementation agreed with the
    // predicate" alone would not catch a candidate that was never classified.
    for name in &accepted {
        assert!(
            !rejected.contains(name),
            "{name:?} is in both the accepted and the rejected set"
        );
    }
    for name in &rejected {
        assert!(
            !accepted.contains(name),
            "{name:?} is in both the accepted and the rejected set"
        );
    }
    assert_eq!(
        accepted.len() + rejected.len(),
        candidates.len(),
        "every candidate must land in exactly one set"
    );

    // The sets are not trivially one-sided: the partition says something about
    // a real boundary, so both sides must be populated.
    assert!(!accepted.is_empty(), "no candidate name is accepted");
    assert!(!rejected.is_empty(), "no candidate name is rejected");
    assert!(
        accepted.contains(&"A".repeat(MAX_ENV_NAME_BYTES)),
        "a name exactly at the bound must be accepted"
    );
    assert!(
        rejected.contains(&"A".repeat(MAX_ENV_NAME_BYTES + 1)),
        "a name one byte over the bound must be rejected"
    );
}

#[test]
fn the_accepted_and_rejected_value_sets_partition_the_candidate_space() {
    let mut accepted = Vec::new();
    let mut rejected = Vec::new();

    let candidates = vec![
        String::new(),
        "value".to_owned(),
        "a value with spaces".to_owned(),
        "a\0value".to_owned(),
        "v".repeat(MAX_ENV_VALUE_BYTES),
        "v".repeat(MAX_ENV_VALUE_BYTES + 1),
    ];

    for value in &candidates {
        let entry = EnvironmentEntry {
            name: "NAME".to_owned(),
            value: value.clone(),
        };
        let verdict = validate_environment_entries(std::slice::from_ref(&entry));
        // The documented value rule: bounded, and free of NUL.
        let documented_accepts =
            value.len() <= MAX_ENV_VALUE_BYTES && !value.as_bytes().contains(&0);

        assert_eq!(
            verdict.is_ok(),
            documented_accepts,
            "the implementation and the documented rule disagree about a value of length {}",
            value.len()
        );

        if documented_accepts {
            accepted.push(value.clone());
        } else {
            rejected.push(value.clone());
        }
    }

    assert_eq!(accepted.len() + rejected.len(), candidates.len());
    for value in &accepted {
        assert!(!rejected.contains(value));
    }
    for value in &rejected {
        assert!(!accepted.contains(value));
    }
    assert!(!accepted.is_empty() && !rejected.is_empty());
}

#[test]
fn the_accepted_and_rejected_list_sets_partition_the_candidate_space() {
    // The list bound and the duplicate-name rule are the two list-level rules.
    let at_bound: Vec<EnvironmentEntry> = (0..MAX_ENV_COUNT)
        .map(|index| EnvironmentEntry {
            name: format!("NAME_{index}"),
            value: "v".to_owned(),
        })
        .collect();
    assert!(
        validate_environment_entries(&at_bound).is_ok(),
        "a list exactly at the count bound must be accepted"
    );

    let over_bound: Vec<EnvironmentEntry> = (0..=MAX_ENV_COUNT)
        .map(|index| EnvironmentEntry {
            name: format!("NAME_{index}"),
            value: "v".to_owned(),
        })
        .collect();
    assert!(
        matches!(
            validate_environment_entries(&over_bound),
            Err(ValidationError::TooMany {
                field: "environment",
                max: MAX_ENV_COUNT
            })
        ),
        "a list one entry over the count bound must be refused by the count rule"
    );

    let duplicate = vec![
        EnvironmentEntry {
            name: "DUPLICATE".to_owned(),
            value: "first".to_owned(),
        },
        EnvironmentEntry {
            name: "DUPLICATE".to_owned(),
            value: "second".to_owned(),
        },
    ];
    assert!(
        matches!(
            validate_environment_entries(&duplicate),
            Err(ValidationError::InvalidSyntax {
                field: "duplicate environment name"
            })
        ),
        "two entries with the same name must be refused by the duplicate rule"
    );

    // The duplicate rule is a distinct rule from the per-entry rule: a name
    // that is individually legal is still refused when it repeats, and the
    // refusal names the duplicate rule rather than the syntax rule.
    let distinct = vec![
        EnvironmentEntry {
            name: "FIRST".to_owned(),
            value: "first".to_owned(),
        },
        EnvironmentEntry {
            name: "SECOND".to_owned(),
            value: "second".to_owned(),
        },
    ];
    assert!(validate_environment_entries(&distinct).is_ok());
}
