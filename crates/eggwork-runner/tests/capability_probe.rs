//! Capability advertisement is backed by the same trust check the runner uses at
//! execution time, so this test has to present the helper the way an
//! installation does: owner-only directory, executable file.
//!
//! The staging rules live in one place, shared with the Landlock suite, because
//! two copies of "copy the built helper somewhere trustable" is exactly how a
//! fixture starts overwriting an executable another test may be running
//! (Operations M004a). The `#[path]` include is deliberate: the alternative is a
//! dev-only workspace member, and the release contract enumerates workspace
//! members.

use eggwork_runner::{LocalProcessRunner, TrustedLandlockSetup};
use std::path::PathBuf;

// One shared fixture surface, two suites: this one only needs `stage` and
// `path`, so the rest would be dead code here and in use next door.
#[allow(dead_code)]
#[cfg(target_os = "linux")]
#[path = "../../eggwork-sandbox-helper/tests/support/helper_fixture.rs"]
mod helper_fixture;

#[cfg(target_os = "linux")]
fn built_helper() -> Option<PathBuf> {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("OUT_DIR").map(PathBuf::from))?;
    let candidate = target.join("debug").join("eggwork-sandbox-helper");
    candidate.exists().then_some(candidate)
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn trusted_helper_advertises_landlock_workspace_rw_capability() {
    let Some(source) = built_helper() else {
        // The helper binary only exists once it has been built. CI builds it
        // before running this suite, so this skip is a local-only convenience.
        eprintln!("eggwork-sandbox-helper is not built; skipping");
        return;
    };
    let fixture = helper_fixture::TrustedHelperFixture::stage(&source);
    let runner = LocalProcessRunner::new(TrustedLandlockSetup::new(fixture.path()));
    let capabilities = runner.execution_capabilities().await;
    assert!(
        capabilities
            .iter()
            .any(|feature| feature == "isolation.landlock.workspace-rw.v1"),
        "expected Landlock workspace-rw capability on a kernel with ABI V4; got {capabilities:?}"
    );
}

/// Platform-neutral negative case: a helper that is not there must not produce
/// a capability. On Windows there is no Landlock at all, so the assertion holds
/// there for a stronger reason than on Linux.
#[tokio::test]
async fn missing_helper_advertises_no_landlock_capability() {
    let runner = LocalProcessRunner::new(TrustedLandlockSetup::new(
        "/nonexistent/eggwork-sandbox-helper",
    ));
    let capabilities = runner.execution_capabilities().await;
    assert!(
        !capabilities
            .iter()
            .any(|feature| feature == "isolation.landlock.workspace-rw.v1"),
        "missing helper must not advertise Landlock capability; got {capabilities:?}"
    );
}
