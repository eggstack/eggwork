//! The declared-artifact capture disposition, checked portably.
//!
//! This target exists because the server's own `#[cfg(test)]` harness is
//! Linux-shaped: it starts `/bin/sh`, uses `std::os::unix`, and sets
//! Unix-only file modes, so it does not compile for a Windows host. The
//! contract this milestone establishes — a declared output that was not
//! captured must never be reported as a successful execution — is a contract
//! about the *decision*, not about the host, so it is checked here against
//! the crate's public API on every platform.
//!
//! Nothing in this file may use `/bin/sh`, `std::os::unix`, or a Unix-only
//! mode: the target is compiled and run by the Windows job in CI.

use eggwork_core::{ArtifactCaptureDisposition, ExecutionFailure, ExecutionState};
use eggwork_server::{
    ArtifactCaptureAttempt, ArtifactCaptureCapability, ArtifactCaptureFacts,
    artifact_capture_capability, decide_artifact_capture,
};

/// The disposition has the required states and round-trips on the wire.
///
/// The spellings are a wire contract: a caller that reads the terminal
/// snapshot has to be able to name the state it is looking at.
#[test]
fn the_capture_disposition_has_the_required_states_and_round_trips() {
    for (disposition, spelling) in [
        (ArtifactCaptureDisposition::NotRequested, "not_requested"),
        (ArtifactCaptureDisposition::Completed, "completed"),
        (ArtifactCaptureDisposition::Failed, "failed"),
    ] {
        let encoded = serde_json::to_string(&disposition).expect("the disposition serializes");
        assert_eq!(
            encoded,
            format!("\"{spelling}\""),
            "the wire spelling of the disposition is a contract"
        );
        let decoded: ArtifactCaptureDisposition =
            serde_json::from_str(&encoded).expect("the disposition deserializes");
        assert_eq!(decoded, disposition);
    }
    // "Not requested" and "failed" are distinguishable from each other, and
    // both are distinguishable from "completed": a caller never has to
    // compare an artifact count against a declared-output count it does not
    // have.
    assert_ne!(
        ArtifactCaptureDisposition::NotRequested,
        ArtifactCaptureDisposition::Failed
    );
    assert_ne!(
        ArtifactCaptureDisposition::Completed,
        ArtifactCaptureDisposition::Failed
    );
}

/// The runtime capture-capability fact reports the correct value for the
/// platform it is running on.
///
/// The fact is the node's, not the caller's: a caller asks the node's platform
/// rather than inferring the answer from its own `cfg`, because the two are
/// not necessarily the same one. On a platform that cannot open a file beneath
/// a workspace root safely, the fact says so.
#[test]
fn the_capture_capability_fact_matches_the_platform() {
    let capability = artifact_capture_capability();
    if cfg!(unix) {
        assert_eq!(
            capability,
            ArtifactCaptureCapability::Available,
            "a Unix platform opens files beneath the workspace root with openat, \
             so it can capture declared outputs"
        );
    } else {
        assert_eq!(
            capability,
            ArtifactCaptureCapability::Unavailable,
            "a platform without a descriptor-relative open cannot capture \
             declared outputs safely, and the fact has to say so"
        );
    }
}

/// A declared output that was not captured is never reported as a success.
///
/// This is the regression the corrective exists for. Before it, the finalization
/// set the finalization-failure marker and left the terminal state
/// `Succeeded`, so a caller that declared outputs saw a successful execution
/// with zero artifacts and no error: the declared outputs were silently
/// dropped. The decision is checked here from outside the crate, so the
/// contract is reachable on a platform where the capture cannot run at all.
#[test]
fn a_declared_output_that_was_not_captured_never_reports_success() {
    let capability = artifact_capture_capability();
    for attempt in [
        ArtifactCaptureAttempt::NotAttempted,
        ArtifactCaptureAttempt::Failed,
    ] {
        let decision = decide_artifact_capture(ArtifactCaptureFacts {
            capability,
            declared_outputs: true,
            target_state: ExecutionState::Succeeded,
            target_failure: None,
            attempt,
        });
        assert_ne!(
            decision.state,
            ExecutionState::Succeeded,
            "an execution whose declared outputs were not captured must not \
             report success"
        );
        assert_eq!(decision.state, ExecutionState::Failed);
        assert_eq!(
            decision.failure,
            Some(ExecutionFailure::Internal),
            "the capture failure is the node's own failure, not the target's"
        );
        assert_eq!(
            decision.disposition,
            ArtifactCaptureDisposition::Failed,
            "a declared output that was not captured must be reported as a \
             failed capture, never as a completed one"
        );
    }
}

/// A platform that cannot capture fails a declared output even when the
/// attempt claims otherwise.
///
/// The capability decides the disposition before the attempt does: a node
/// whose platform cannot capture safely has no completed capture to report,
/// so the declared output is refused rather than dropped.
#[test]
fn a_platform_that_cannot_capture_refuses_a_declared_output() {
    let decision = decide_artifact_capture(ArtifactCaptureFacts {
        capability: ArtifactCaptureCapability::Unavailable,
        declared_outputs: true,
        target_state: ExecutionState::Succeeded,
        target_failure: None,
        attempt: ArtifactCaptureAttempt::Completed,
    });
    assert_ne!(decision.state, ExecutionState::Succeeded);
    assert_eq!(decision.disposition, ArtifactCaptureDisposition::Failed);
}

/// An execution that declared nothing reports that nothing was requested.
///
/// The disposition distinguishes "no declared outputs were requested" from
/// "declared outputs were requested and the capture did not complete", so a
/// caller never has to compare the artifact count against a declared-output
/// count it does not have.
#[test]
fn an_execution_without_declared_outputs_reports_not_requested() {
    let decision = decide_artifact_capture(ArtifactCaptureFacts {
        capability: artifact_capture_capability(),
        declared_outputs: false,
        target_state: ExecutionState::Succeeded,
        target_failure: None,
        attempt: ArtifactCaptureAttempt::NotAttempted,
    });
    assert_eq!(decision.state, ExecutionState::Succeeded);
    assert_eq!(decision.failure, None);
    assert_eq!(
        decision.disposition,
        ArtifactCaptureDisposition::NotRequested
    );
}

/// A completed capture on a capable platform leaves the target's outcome alone.
///
/// The fail-closed rule must not swallow the success it is meant to protect:
/// a capture that completed keeps the target's own terminal state and
/// failure, and reports a completed disposition.
#[test]
fn a_completed_capture_keeps_the_targets_own_outcome() {
    let decision = decide_artifact_capture(ArtifactCaptureFacts {
        capability: ArtifactCaptureCapability::Available,
        declared_outputs: true,
        target_state: ExecutionState::Succeeded,
        target_failure: None,
        attempt: ArtifactCaptureAttempt::Completed,
    });
    assert_eq!(decision.state, ExecutionState::Succeeded);
    assert_eq!(decision.failure, None);
    assert_eq!(decision.disposition, ArtifactCaptureDisposition::Completed);
}

/// A target that failed keeps its own failure when the capture also fails.
///
/// A caller has to be able to tell "the target failed" from "the target
/// succeeded and its declared outputs could not be collected", so the
/// capture failure never overwrites the target's own failure.
#[test]
fn a_failed_target_keeps_its_own_failure_when_the_capture_also_fails() {
    let decision = decide_artifact_capture(ArtifactCaptureFacts {
        capability: ArtifactCaptureCapability::Available,
        declared_outputs: true,
        target_state: ExecutionState::Failed,
        target_failure: Some(ExecutionFailure::Spawn),
        attempt: ArtifactCaptureAttempt::Failed,
    });
    assert_eq!(decision.state, ExecutionState::Failed);
    assert_eq!(
        decision.failure,
        Some(ExecutionFailure::Spawn),
        "the target's own failure must survive a capture failure"
    );
    assert_eq!(decision.disposition, ArtifactCaptureDisposition::Failed);
}
